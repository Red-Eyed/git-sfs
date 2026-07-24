//! `git-sfs status`: report tracked objects without moving bytes.
//!
//! `status` is deliberately observational. Missing files are data to report,
//! not a failure class, and remote lookup failures become `unknown` in the
//! report rather than being collapsed into "absent" — the v1 defect called out
//! in rust-rewrite-plan §2.5 and contract-spec §13.3.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::error::Error;
use crate::ports::{Remote, RemoteError, Repo, RepoError, ScannedEntry, Store, StoreError};

/// Size is unknown: absent locally and absent or unknown remotely.
pub const SIZE_UNKNOWN: i64 = -1;

/// The complete status report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// Number of tracked symlinks in scope.
    pub tracked: usize,
    /// Number of distinct object hashes those links reference.
    pub unique_files: usize,
    /// Distinct objects present in the local cache.
    pub cached: usize,
    /// Distinct objects absent from the local cache.
    pub missing_local: usize,
    /// Total known bytes, counting each unique object once.
    pub total_size: u64,
    /// Whether remote metadata was requested.
    pub remote_checked: bool,
    /// Distinct objects confirmed present remotely, only when checked.
    pub on_remote: Option<usize>,
    /// Distinct objects confirmed absent remotely, only when checked.
    pub unpushed: Option<usize>,
    /// Distinct objects whose remote state could not be determined, only when
    /// checked.
    pub remote_unknown: Option<usize>,
    /// One row per tracked symlink, sorted by path.
    pub files: Vec<StatusFile>,
}

/// One tracked symlink in a status report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFile {
    /// Repo-relative symlink path.
    pub path: Utf8PathBuf,
    /// The hash the symlink references.
    pub hash: Sha256,
    /// Known size in bytes, or [`SIZE_UNKNOWN`].
    pub size: i64,
    /// Whether the object is present in the local cache.
    pub cached: bool,
    /// Remote state, absent when no remote check ran.
    pub remote: Option<RemoteState>,
}

/// Remote metadata state for one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteState {
    /// The remote confirmed the object is present.
    Present,
    /// The remote confirmed the object is absent.
    Absent,
    /// The remote lookup failed, so presence is unknown.
    Unknown {
        /// The cause rendered for display.
        cause: String,
    },
}

/// Why `status` could not produce a report.
#[derive(Debug, Error)]
pub enum StatusError {
    /// The repository scan failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// Local cache metadata could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Remote metadata lookup was canceled.
    #[error(transparent)]
    Remote(#[from] RemoteError),
}

impl From<StatusError> for Error {
    fn from(err: StatusError) -> Self {
        match err {
            StatusError::Repo(RepoError::Canceled)
            | StatusError::Store(StoreError::Canceled)
            | StatusError::Remote(RemoteError::Canceled) => Error::Canceled,
            StatusError::Repo(err) => Error::from(err),
            StatusError::Store(err) => Error::from(err),
            StatusError::Remote(err) => Error::from(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectState {
    size: Option<u64>,
    cached: bool,
    remote: Option<RemoteState>,
}

/// Builds a status report for every tracked link at or below `scope`.
///
/// Invalid or unrepresentable symlinks are ignored here, matching v1's
/// `collectGitSFSSymlinks` behavior for `status`; `verify` reports them.
///
/// # Errors
///
/// Returns [`StatusError`] if the repo scan or local cache metadata query
/// fails. Remote metadata failures are represented inside the report as
/// [`RemoteState::Unknown`].
pub fn status(
    repo: &dyn Repo,
    store: &dyn Store,
    remote: Option<&dyn Remote>,
    scope: &Utf8Path,
    cancel: &Cancel,
) -> Result<StatusReport, StatusError> {
    let links = tracked_links(repo.scan(scope, cancel)?);
    let hashes = unique_hashes(&links);
    let states = inspect_objects(store, remote, &hashes, cancel)?;
    Ok(build_report(links, states, remote.is_some()))
}

fn tracked_links(entries: Vec<ScannedEntry>) -> Vec<(Utf8PathBuf, Sha256)> {
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            ScannedEntry::Tracked { path, hash } => Some((path, hash)),
            ScannedEntry::Invalid { .. } | ScannedEntry::Unrepresentable { .. } => None,
        })
        .collect()
}

fn unique_hashes(links: &[(Utf8PathBuf, Sha256)]) -> Vec<Sha256> {
    links
        .iter()
        .map(|(_, hash)| *hash)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn inspect_objects(
    store: &dyn Store,
    remote: Option<&dyn Remote>,
    hashes: &[Sha256],
    cancel: &Cancel,
) -> Result<BTreeMap<Sha256, ObjectState>, StatusError> {
    let remote_sizes = match remote {
        Some(remote) => match remote.file_sizes(hashes, cancel) {
            Ok(sizes) => RemoteLookup::Known(sizes),
            Err(RemoteError::Canceled) => return Err(StatusError::Remote(RemoteError::Canceled)),
            Err(err) => RemoteLookup::Unknown(err.to_string()),
        },
        None => RemoteLookup::Unchecked,
    };

    let mut states = BTreeMap::new();
    for &hash in hashes {
        let local_size = store.object_size(hash)?;
        let remote_state = remote_sizes.state_for(hash);
        let size = local_size.or_else(|| remote_sizes.size_for(hash));
        states.insert(
            hash,
            ObjectState {
                size,
                cached: local_size.is_some(),
                remote: remote_state,
            },
        );
    }
    Ok(states)
}

enum RemoteLookup {
    Unchecked,
    Known(HashMap<Sha256, u64>),
    Unknown(String),
}

impl RemoteLookup {
    fn state_for(&self, hash: Sha256) -> Option<RemoteState> {
        match self {
            Self::Unchecked => None,
            Self::Known(sizes) if sizes.contains_key(&hash) => Some(RemoteState::Present),
            Self::Known(_) => Some(RemoteState::Absent),
            Self::Unknown(cause) => Some(RemoteState::Unknown {
                cause: cause.clone(),
            }),
        }
    }

    fn size_for(&self, hash: Sha256) -> Option<u64> {
        match self {
            Self::Known(sizes) => sizes.get(&hash).copied(),
            Self::Unchecked | Self::Unknown(_) => None,
        }
    }
}

fn build_report(
    links: Vec<(Utf8PathBuf, Sha256)>,
    states: BTreeMap<Sha256, ObjectState>,
    remote_checked: bool,
) -> StatusReport {
    let mut files = Vec::with_capacity(links.len());
    for (path, hash) in links {
        let state = states
            .get(&hash)
            .expect("every link hash has an inspected object state");
        files.push(StatusFile {
            path,
            hash,
            size: state.size.map_or(SIZE_UNKNOWN, |size| size as i64),
            cached: state.cached,
            remote: state.remote.clone(),
        });
    }

    let mut cached = 0usize;
    let mut total_size = 0u64;
    let mut on_remote = 0usize;
    let mut unpushed = 0usize;
    let mut remote_unknown = 0usize;
    for state in states.values() {
        if state.cached {
            cached += 1;
        }
        if let Some(size) = state.size {
            total_size += size;
        }
        match state.remote {
            Some(RemoteState::Present) => on_remote += 1,
            Some(RemoteState::Absent) => unpushed += 1,
            Some(RemoteState::Unknown { .. }) => remote_unknown += 1,
            None => {}
        }
    }

    let unique_files = states.len();
    StatusReport {
        tracked: files.len(),
        unique_files,
        cached,
        missing_local: unique_files - cached,
        total_size,
        remote_checked,
        on_remote: remote_checked.then_some(on_remote),
        unpushed: remote_checked.then_some(unpushed),
        remote_unknown: remote_checked.then_some(remote_unknown),
        files,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use crate::domain::hash::ALGORITHM;
    use crate::domain::symlink::git_link_target;
    use crate::ports::{FakeRemote, FakeRepo, FakeStore};

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

    fn remote_rel_path(hash: Sha256) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash.to_hex()))
    }

    fn remote_store_bytes(remote: &FakeRemote, hash: Sha256, bytes: &[u8], cancel: &Cancel) {
        let dir = tempfile::tempdir().unwrap();
        let files_dir = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let rel = remote_rel_path(hash);
        let src = files_dir.join(&rel);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, bytes).unwrap();
        remote.copy_to_remote(&files_dir, &[rel], cancel).unwrap();
    }

    #[test]
    fn local_status_counts_unique_objects_but_reports_every_link() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let cancel = Cancel::new();
        let cached_hash = hash_bytes(&[1; 12]);
        seed_link(&repo, "a.bin", cached_hash);
        seed_link(&repo, "copy.bin", cached_hash);
        seed_link(&repo, "missing.bin", hash(2));
        store_bytes(&store, cached_hash, &[1; 12]);

        let report = status(&repo, &store, None, Utf8Path::new("."), &cancel).unwrap();

        assert_eq!(report.tracked, 3);
        assert_eq!(report.unique_files, 2);
        assert_eq!(report.cached, 1);
        assert_eq!(report.missing_local, 1);
        assert_eq!(report.total_size, 12);
        assert!(!report.remote_checked);
        assert_eq!(report.files[2].size, SIZE_UNKNOWN);
        assert!(report.files.iter().all(|file| file.remote.is_none()));
    }

    #[test]
    fn remote_status_uses_remote_size_when_local_cache_is_missing() {
        let repo = FakeRepo::new("/repo");
        let remote = FakeRemote::new();
        let cancel = Cancel::new();
        seed_link(&repo, "remote.bin", hash(3));
        remote_store_bytes(&remote, hash(3), &[3; 9], &cancel);

        let empty_store = FakeStore::new();
        let report = status(
            &repo,
            &empty_store,
            Some(&remote),
            Utf8Path::new("."),
            &cancel,
        )
        .unwrap();

        assert_eq!(report.cached, 0);
        assert_eq!(report.total_size, 9);
        assert_eq!(report.on_remote, Some(1));
        assert_eq!(report.unpushed, Some(0));
        assert_eq!(report.files[0].remote, Some(RemoteState::Present));
    }

    #[test]
    fn remote_lookup_failure_is_reported_as_unknown_not_absent() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let cancel = Cancel::new();
        seed_link(&repo, "a.bin", hash(4));
        remote.set_unreachable();

        let report = status(&repo, &store, Some(&remote), Utf8Path::new("."), &cancel).unwrap();

        assert_eq!(report.on_remote, Some(0));
        assert_eq!(report.unpushed, Some(0));
        assert_eq!(report.remote_unknown, Some(1));
        assert!(matches!(
            report.files[0].remote,
            Some(RemoteState::Unknown { .. })
        ));
    }
}
