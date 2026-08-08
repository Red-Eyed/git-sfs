//! `git-sfs verify`: strict repository integrity checks.
//!
//! This command is the CI gate. It reports every issue it can classify and
//! returns non-zero when any issue is present. Remote work stays batch-shaped:
//! metadata uses one [`Remote::file_sizes`] call, and `--with-integrity`
//! downloads the whole verification set into a scratch object tree with one
//! [`Remote::copy_from_remote`] call before hashing locally.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::{ALGORITHM, Sha256};
use crate::error::Error;
use crate::ports::{
    Remote, RemoteError, Repo, RepoError, ScannedEntry, Store, StoreError, hash_file,
};

/// Issue kinds reported by `verify`, in stable display order.
pub const ISSUE_KINDS: &[IssueKind] = &[
    IssueKind::UnconvertedFile,
    IssueKind::BrokenGitSymlink,
    IssueKind::MissingCacheFile,
    IssueKind::CorruptCacheFile,
    IssueKind::WrongCachePermissions,
    IssueKind::MissingRemoteFile,
    IssueKind::CorruptRemoteFile,
    IssueKind::InvalidConfig,
];

/// The complete verify report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Number of valid tracked symlinks in scope.
    pub tracked_symlinks: usize,
    /// Number of orphaned cache objects. Orphans are advisory, not failures.
    pub orphan_count: usize,
    /// Every issue found, sorted in scan/object order.
    pub issues: Vec<VerifyIssue>,
}

impl VerifyReport {
    /// Whether this report should make `verify` fail.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        !self.issues.is_empty()
    }

    /// Count issues by kind.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<IssueKind, usize> {
        let mut counts = BTreeMap::new();
        for issue in &self.issues {
            *counts.entry(issue.kind).or_insert(0) += 1;
        }
        counts
    }
}

/// One verify problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyIssue {
    /// The issue kind.
    pub kind: IssueKind,
    /// Repo-relative path associated with the issue, when one exists.
    pub path: Option<Utf8PathBuf>,
    /// Object hash associated with the issue, when one exists.
    pub hash: Option<Sha256>,
    /// Human-readable detail.
    pub detail: Option<String>,
}

impl VerifyIssue {
    fn new(kind: IssueKind) -> Self {
        Self {
            kind,
            path: None,
            hash: None,
            detail: None,
        }
    }

    fn path(mut self, path: Utf8PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    fn hash(mut self, hash: Sha256) -> Self {
        self.hash = Some(hash);
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Verify issue categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IssueKind {
    /// Reserved for compatibility; verify deliberately does not fail on
    /// unrelated regular files because false reds weaken a CI gate.
    UnconvertedFile,
    /// A symlink candidate exists but does not validate as a git-sfs link.
    BrokenGitSymlink,
    /// A referenced object is absent from the local cache.
    MissingCacheFile,
    /// A referenced local cache object hashes to the wrong bytes.
    CorruptCacheFile,
    /// Reserved for compatibility; intact writable objects are repaired
    /// through [`Store::verified`].
    WrongCachePermissions,
    /// A referenced object is absent from the remote.
    MissingRemoteFile,
    /// A referenced remote object is present but corrupt or size-mismatched.
    CorruptRemoteFile,
    /// The repository configuration is invalid.
    InvalidConfig,
}

impl IssueKind {
    /// Singular display text.
    #[must_use]
    pub fn singular(self) -> &'static str {
        match self {
            Self::UnconvertedFile => "unconverted file",
            Self::BrokenGitSymlink => "broken git symlink",
            Self::MissingCacheFile => "missing cache file",
            Self::CorruptCacheFile => "corrupt cache file",
            Self::WrongCachePermissions => "wrong cache permissions",
            Self::MissingRemoteFile => "missing remote file",
            Self::CorruptRemoteFile => "corrupt remote file",
            Self::InvalidConfig => "invalid config",
        }
    }

    /// Plural display text.
    #[must_use]
    pub fn plural(self) -> &'static str {
        match self {
            Self::UnconvertedFile => "unconverted files",
            Self::BrokenGitSymlink => "broken git symlinks",
            Self::MissingCacheFile => "missing cache files",
            Self::CorruptCacheFile => "corrupt cache files",
            Self::WrongCachePermissions => "wrong cache permissions",
            Self::MissingRemoteFile => "missing remote files",
            Self::CorruptRemoteFile => "corrupt remote files",
            Self::InvalidConfig => "invalid config",
        }
    }
}

/// Why `verify` could not complete.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// The repository scan failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// Local cache inspection failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Remote metadata or transfer failed.
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// Scratch-space setup for remote integrity verification failed.
    #[error("{path}: {source}")]
    Scratch {
        /// The path being prepared or read.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Verification completed and found reportable problems.
    #[error("verify failed with {count} issue(s)")]
    Failed {
        /// Number of report issues.
        count: usize,
        /// The completed report.
        report: VerifyReport,
    },
}

impl VerifyError {
    /// Returns the report carried by a completed failed verification.
    #[must_use]
    pub fn report(&self) -> Option<&VerifyReport> {
        match self {
            Self::Failed { report, .. } => Some(report),
            Self::Repo(_) | Self::Store(_) | Self::Remote(_) | Self::Scratch { .. } => None,
        }
    }
}

impl From<VerifyError> for Error {
    fn from(err: VerifyError) -> Self {
        match err {
            VerifyError::Repo(RepoError::Canceled)
            | VerifyError::Store(StoreError::Canceled)
            | VerifyError::Remote(RemoteError::Canceled) => Error::Canceled,
            VerifyError::Repo(err) => Error::from(err),
            VerifyError::Store(err) => Error::from(err),
            VerifyError::Remote(err) => Error::from(err),
            VerifyError::Scratch { .. } => Error::Unavailable(err.to_string()),
            VerifyError::Failed { report, .. } => classify_failed_report(&report),
        }
    }
}

/// Runs strict verification for every git-sfs symlink at or below `scope`.
///
/// # Errors
///
/// Returns [`VerifyError::Failed`] with a report when the scan completed and
/// found issues. Other variants mean the command could not determine the
/// answer and should not be rendered as a report full of absences.
pub fn verify(
    repo: &dyn Repo,
    store: &dyn Store,
    remote: Option<&dyn Remote>,
    remote_scratch_root: &Utf8Path,
    scope: &Utf8Path,
    with_integrity: bool,
    cancel: &Cancel,
) -> Result<VerifyReport, VerifyError> {
    let entries = repo.scan(scope, cancel)?;
    let scanned = classify_scan(entries);
    let hashes = unique_hashes(&scanned.links);
    let mut issues = scanned.issues;

    let local = inspect_local(store, &hashes, with_integrity, cancel)?;
    issues.extend(local_issues(&scanned.links, &local));

    if let Some(remote) = remote {
        let remote_sizes = remote.file_sizes(&hashes, cancel)?;
        let remote_presence = remote_presence_issues(&scanned.links, &local, &remote_sizes);
        let size_mismatches = remote_presence
            .iter()
            .filter_map(|issue| (issue.kind == IssueKind::CorruptRemoteFile).then_some(issue.hash))
            .flatten()
            .collect::<BTreeSet<_>>();
        issues.extend(remote_presence);
        if with_integrity {
            issues.extend(remote_integrity_issues(
                remote,
                remote_scratch_root,
                &hashes,
                &remote_sizes,
                &size_mismatches,
                cancel,
            )?);
        }
    }

    let orphan_count = count_orphans(store, &hashes)?;
    let report = VerifyReport {
        tracked_symlinks: scanned.links.len(),
        orphan_count,
        issues,
    };
    if report.has_failures() {
        return Err(VerifyError::Failed {
            count: report.issues.len(),
            report,
        });
    }
    Ok(report)
}

#[derive(Debug)]
struct Scanned {
    links: Vec<TrackedLink>,
    issues: Vec<VerifyIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedLink {
    path: Utf8PathBuf,
    hash: Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalState {
    Present { size: u64 },
    Missing,
    Corrupt(String),
}

fn classify_scan(entries: Vec<ScannedEntry>) -> Scanned {
    let mut links = Vec::new();
    let mut issues = Vec::new();
    for entry in entries {
        match entry {
            ScannedEntry::Tracked { path, hash } => links.push(TrackedLink { path, hash }),
            ScannedEntry::Invalid { path, reason } => {
                issues.push(
                    VerifyIssue::new(IssueKind::BrokenGitSymlink)
                        .path(path)
                        .detail(reason.to_string()),
                );
            }
            ScannedEntry::Unrepresentable { description } => {
                issues.push(
                    VerifyIssue::new(IssueKind::BrokenGitSymlink)
                        .detail(format!("path is not valid UTF-8: {description}")),
                );
            }
        }
    }
    Scanned { links, issues }
}

fn unique_hashes(links: &[TrackedLink]) -> Vec<Sha256> {
    links
        .iter()
        .map(|link| link.hash)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn inspect_local(
    store: &dyn Store,
    hashes: &[Sha256],
    with_integrity: bool,
    cancel: &Cancel,
) -> Result<HashMap<Sha256, LocalState>, VerifyError> {
    let mut states = HashMap::with_capacity(hashes.len());
    for &hash in hashes {
        let entry = if with_integrity {
            store.rehash_object(hash, cancel)
        } else {
            store.verified(hash, cancel)
        };
        let state = match entry {
            Ok(Some(_)) => LocalState::Present {
                size: store.object_size(hash)?.unwrap_or(0),
            },
            Ok(None) => LocalState::Missing,
            Err(error @ StoreError::HashMismatch { .. }) => LocalState::Corrupt(error.to_string()),
            Err(error) => return Err(VerifyError::Store(error)),
        };
        states.insert(hash, state);
    }
    Ok(states)
}

fn local_issues(links: &[TrackedLink], states: &HashMap<Sha256, LocalState>) -> Vec<VerifyIssue> {
    links
        .iter()
        .filter_map(|link| match states.get(&link.hash) {
            Some(LocalState::Missing) => Some(
                VerifyIssue::new(IssueKind::MissingCacheFile)
                    .path(link.path.clone())
                    .hash(link.hash),
            ),
            Some(LocalState::Corrupt(detail)) => Some(
                VerifyIssue::new(IssueKind::CorruptCacheFile)
                    .path(link.path.clone())
                    .hash(link.hash)
                    .detail(detail.clone()),
            ),
            Some(LocalState::Present { .. }) | None => None,
        })
        .collect()
}

fn remote_presence_issues(
    links: &[TrackedLink],
    local: &HashMap<Sha256, LocalState>,
    remote_sizes: &HashMap<Sha256, u64>,
) -> Vec<VerifyIssue> {
    links
        .iter()
        .filter_map(|link| {
            let Some(remote_size) = remote_sizes.get(&link.hash) else {
                return Some(
                    VerifyIssue::new(IssueKind::MissingRemoteFile)
                        .path(link.path.clone())
                        .hash(link.hash),
                );
            };
            match local.get(&link.hash) {
                Some(LocalState::Present { size }) if size != remote_size => Some(
                    VerifyIssue::new(IssueKind::CorruptRemoteFile)
                        .path(link.path.clone())
                        .hash(link.hash)
                        .detail(format!(
                            "remote size {remote_size} does not match local size {size}"
                        )),
                ),
                Some(LocalState::Present { .. } | LocalState::Missing | LocalState::Corrupt(_))
                | None => None,
            }
        })
        .collect()
}

fn remote_integrity_issues(
    remote: &dyn Remote,
    scratch_root: &Utf8Path,
    hashes: &[Sha256],
    remote_sizes: &HashMap<Sha256, u64>,
    size_mismatches: &BTreeSet<Sha256>,
    cancel: &Cancel,
) -> Result<Vec<VerifyIssue>, VerifyError> {
    let present = hashes
        .iter()
        .copied()
        .filter(|hash| remote_sizes.contains_key(hash) && !size_mismatches.contains(hash))
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(scratch_root).map_err(|source| VerifyError::Scratch {
        path: scratch_root.to_owned(),
        source,
    })?;
    let scratch = tempfile::Builder::new()
        .prefix("git-sfs-verify-")
        .tempdir_in(scratch_root)
        .map_err(|source| VerifyError::Scratch {
            path: scratch_root.to_owned(),
            source,
        })?;
    let files_dir = Utf8PathBuf::from_path_buf(scratch.path().join("files")).map_err(|path| {
        VerifyError::Scratch {
            path: Utf8PathBuf::from(path.to_string_lossy().into_owned()),
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "scratch path is not valid UTF-8",
            ),
        }
    })?;
    std::fs::create_dir_all(&files_dir).map_err(|source| VerifyError::Scratch {
        path: files_dir.clone(),
        source,
    })?;
    let rel_paths = present
        .iter()
        .map(|hash| remote_rel_path(*hash))
        .collect::<Vec<_>>();
    remote.copy_from_remote(&files_dir, &rel_paths, cancel)?;

    let mut issues = Vec::new();
    for hash in present {
        let path = files_dir.join(remote_rel_path(hash));
        let got = hash_file(&path, cancel).map_err(|source| VerifyError::Scratch {
            path: path.clone(),
            source,
        })?;
        if got != hash {
            issues.push(
                VerifyIssue::new(IssueKind::CorruptRemoteFile)
                    .hash(hash)
                    .detail(format!("remote object hashes to {got}")),
            );
        }
    }
    Ok(issues)
}

fn count_orphans(store: &dyn Store, tracked_hashes: &[Sha256]) -> Result<usize, VerifyError> {
    let tracked = tracked_hashes.iter().copied().collect::<BTreeSet<_>>();
    Ok(store
        .object_hashes()?
        .into_iter()
        .filter(|hash| !tracked.contains(hash))
        .count())
}

fn remote_rel_path(hash: Sha256) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash.to_hex()))
}

fn classify_failed_report(report: &VerifyReport) -> Error {
    let message = format!("verify failed with {} issue(s)", report.issues.len());
    if report.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            IssueKind::CorruptCacheFile
                | IssueKind::WrongCachePermissions
                | IssueKind::CorruptRemoteFile
                | IssueKind::BrokenGitSymlink
        )
    }) {
        Error::Integrity(message)
    } else if report.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            IssueKind::MissingCacheFile | IssueKind::MissingRemoteFile
        )
    }) {
        Error::Missing(message)
    } else {
        Error::Config(message)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use crate::domain::symlink::git_link_target;
    use crate::exec::init as init_cmd;
    use crate::ports::{FakeRemote, FakeRepo, FakeStore, FsRepo, FsStore};

    use super::*;

    fn hash(byte: u8) -> Sha256 {
        Sha256::from_digest([byte; 32])
    }

    fn hash_bytes(bytes: &[u8]) -> Sha256 {
        use sha2::{Digest, Sha256 as Sha256Hasher};
        Sha256::from_digest(Sha256Hasher::digest(bytes).into())
    }

    fn seed_link(repo: &FakeRepo, path: &str, hash: Sha256) {
        let root = Utf8Path::new("/repo");
        let file = root.join(path);
        let target = git_link_target(root, &file, hash).unwrap();
        repo.seed(path, target.to_string());
    }

    fn store_bytes(store: &FakeStore, hash: Sha256, bytes: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("object")).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        store.store(&path, hash, &Cancel::new()).unwrap();
    }

    fn remote_store_bytes(remote: &FakeRemote, hash: Sha256, bytes: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let files_dir = Utf8PathBuf::from_path_buf(dir.path().join("files")).unwrap();
        let rel = remote_rel_path(hash);
        let src = files_dir.join(&rel);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, bytes).unwrap();
        remote
            .copy_to_remote(&files_dir, &[rel], &Cancel::new())
            .unwrap();
    }

    fn scratch_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn reports_missing_local_and_remote_objects() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        seed_link(&repo, "data.bin", hash(1));

        let err = verify(
            &repo,
            &store,
            Some(&remote),
            Utf8Path::new("/cache/tmp"),
            Utf8Path::new("."),
            false,
            &Cancel::new(),
        )
        .unwrap_err();
        let report = err.report().unwrap();

        assert_eq!(report.tracked_symlinks, 1);
        assert!(report.issues.iter().any(|issue| {
            issue.kind == IssueKind::MissingCacheFile
                && issue.path.as_deref() == Some(Utf8Path::new("data.bin"))
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.kind == IssueKind::MissingRemoteFile
                && issue.path.as_deref() == Some(Utf8Path::new("data.bin"))
        }));
    }

    #[test]
    fn intact_objects_are_ok_without_a_remote_check() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let bytes = b"trusted local object";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        store_bytes(&store, hash, bytes);

        let report = verify(
            &repo,
            &store,
            None,
            Utf8Path::new("/cache/tmp"),
            Utf8Path::new("."),
            false,
            &Cancel::new(),
        )
        .unwrap();

        assert!(report.issues.is_empty());
        assert_eq!(report.counts().get(&IssueKind::WrongCachePermissions), None);
    }

    #[test]
    fn verify_check_remote_rejects_a_truncated_remote_object() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let bytes = b"trusted local object";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        store_bytes(&store, hash, bytes);
        remote_store_bytes(&remote, hash, &bytes[..4]);

        let err = verify(
            &repo,
            &store,
            Some(&remote),
            Utf8Path::new("/cache/tmp"),
            Utf8Path::new("."),
            false,
            &Cancel::new(),
        )
        .unwrap_err();

        assert!(err.report().unwrap().issues.iter().any(|issue| {
            issue.kind == IssueKind::CorruptRemoteFile
                && issue.path.as_deref() == Some(Utf8Path::new("data.bin"))
                && issue.hash == Some(hash)
                && issue.detail.as_deref() == Some("remote size 4 does not match local size 20")
        }));
    }

    #[test]
    fn remote_integrity_downloads_batch_and_hashes_remote_bytes() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let bytes = b"remote object";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        remote_store_bytes(&remote, hash, b"same length!!");
        let scratch = scratch_root();
        let scratch_path = Utf8PathBuf::from_path_buf(scratch.path().join("cache-tmp")).unwrap();

        let err = verify(
            &repo,
            &store,
            Some(&remote),
            &scratch_path,
            Utf8Path::new("."),
            true,
            &Cancel::new(),
        )
        .unwrap_err();

        assert!(err.report().unwrap().issues.iter().any(|issue| {
            issue.kind == IssueKind::CorruptRemoteFile && issue.hash == Some(hash)
        }));
        assert!(scratch_path.exists());
    }

    #[test]
    fn freshly_initialized_repo_verifies_without_a_remote_check() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let init_outcome =
            init_cmd::init(&repo, &repo.join(".git-sfs/config.toml"), None, false).unwrap();
        let repo_port = FsRepo::new(repo);
        let store = FsStore::new(init_outcome.cache_root.clone());

        let report = verify(
            &repo_port,
            &store,
            None,
            &init_outcome.cache_root.join("tmp"),
            Utf8Path::new("."),
            false,
            &Cancel::new(),
        )
        .unwrap();

        assert_eq!(report.tracked_symlinks, 0);
        assert_eq!(report.orphan_count, 0);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn orphaned_cache_objects_are_advisory_not_failures() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        store_bytes(&store, hash_bytes(b"orphan"), b"orphan");

        let report = verify(
            &repo,
            &store,
            None,
            Utf8Path::new("/cache/tmp"),
            Utf8Path::new("."),
            false,
            &Cancel::new(),
        )
        .unwrap();

        assert_eq!(report.orphan_count, 1);
        assert!(report.issues.is_empty());
    }
}
