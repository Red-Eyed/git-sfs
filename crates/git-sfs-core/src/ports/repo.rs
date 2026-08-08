//! The repository: walking the tree, reading the symlinks on it.
//!
//! [`Repo::scan`] returns every candidate as data: [`ScannedEntry::Tracked`]
//! for a symlink that validates, [`ScannedEntry::Invalid`] for one that does
//! not, and [`ScannedEntry::Unrepresentable`] for a path git-sfs cannot name
//! losslessly. Each command chooses its own policy: `push`, `pull`, and
//! `status` ignore invalid symlinks; `verify` reports them.
//!
//! Directory-read failures, or the requested scope not existing at all, abort
//! the whole scan with `Err` because git-sfs cannot determine the tree's
//! contents. A single candidate symlink failing validation does not abort.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::domain::symlink::{InvalidSymlinkTarget, clean_utf8, validate_symlink_target};
use crate::error::Error;

/// One candidate [`Repo::scan`] found at or below the requested scope.
#[derive(Debug)]
pub enum ScannedEntry {
    /// A valid git-sfs symlink.
    Tracked {
        /// Repo-relative path to the symlink.
        path: Utf8PathBuf,
        /// The hash its target names.
        hash: Sha256,
    },
    /// A symlink that looked like a candidate but is not a valid git-sfs
    /// link — either unrelated to git-sfs, or a broken/hand-edited one.
    /// `push`/`pull`/`status` ignore these; `verify` reports them.
    Invalid {
        /// Repo-relative path to the symlink.
        path: Utf8PathBuf,
        /// Why it did not validate.
        reason: InvalidReason,
    },
    /// A symlink candidate whose own filename is not valid UTF-8 — skipped
    /// rather than validated, since there is no lossless `Utf8PathBuf` to
    /// name it with (this crate's paths are real UTF-8 throughout, never a
    /// silently-mangled guess). Still reported, not silently dropped, so a
    /// command layer can warn about it: `description` is a best-effort,
    /// display-only rendering of the raw bytes — the only use this value
    /// gets, since there is nothing to act on further.
    Unrepresentable {
        /// A lossy, human-readable rendering of the unreadable path, for a
        /// warning message only.
        description: String,
    },
}

impl ScannedEntry {
    /// This entry's repo-relative path, or `None` for
    /// [`ScannedEntry::Unrepresentable`], which has none.
    #[must_use]
    pub fn path(&self) -> Option<&Utf8Path> {
        match self {
            Self::Tracked { path, .. } | Self::Invalid { path, .. } => Some(path),
            Self::Unrepresentable { .. } => None,
        }
    }
}

/// One regular file [`Repo::find_files`] found at or below the requested
/// scope: `add`'s candidate set.
#[derive(Debug)]
pub enum FoundEntry {
    /// A regular file at this repo-relative path.
    File {
        /// The repo-relative path.
        path: Utf8PathBuf,
        /// Whether Git already tracks this path in the index.
        git_tracked: bool,
    },
    /// A candidate whose own filename is not valid UTF-8 — skipped rather
    /// than returned, for the same reason and with the same "report, don't
    /// silently drop" treatment as [`ScannedEntry::Unrepresentable`].
    Unrepresentable {
        /// A lossy, human-readable rendering of the unreadable path, for a
        /// warning message only.
        description: String,
    },
}

impl FoundEntry {
    /// This entry's repo-relative path, or `None` for
    /// [`FoundEntry::Unrepresentable`], which has none.
    #[must_use]
    pub fn path(&self) -> Option<&Utf8Path> {
        match self {
            Self::File { path, .. } => Some(path),
            Self::Unrepresentable { .. } => None,
        }
    }
}

/// Why [`Repo::scan`] could not validate a candidate symlink as a git-sfs
/// link.
#[derive(Debug, Error)]
pub enum InvalidReason {
    /// `readlink()` itself failed — e.g. permission denied, or the entry
    /// disappeared between being listed and being read.
    #[error("reading symlink: {0}")]
    Unreadable(#[source] std::io::Error),
    /// The target text `readlink()` returned is not valid UTF-8 —
    /// unrepresentable as the `&str` validation operates over. Distinct from
    /// [`ScannedEntry::Unrepresentable`], which is about the symlink's own
    /// *name*, not what it points at.
    #[error("symlink target is not valid UTF-8")]
    TargetNotUtf8,
    /// One of the symlink-target validation rules failed.
    #[error(transparent)]
    InvalidTarget(#[from] InvalidSymlinkTarget),
}

/// Why a [`Repo`] operation failed outright — aborting the whole scan,
/// unlike a single symlink failing validation (see the module doc).
#[derive(Debug, Error)]
pub enum RepoError {
    /// The walk itself could not continue: the requested scope does not
    /// exist, or a directory within it could not be read.
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
    /// Git could not answer an index query.
    #[error("git ls-files failed in {repo}: {detail}")]
    Git {
        /// The repository root.
        repo: Utf8PathBuf,
        /// The command failure details.
        detail: String,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<RepoError> for Error {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::Walk(_) | RepoError::Git { .. } => Error::Unavailable(err.to_string()),
            RepoError::Canceled => Error::Canceled,
        }
    }
}

/// A repository tree: the source of every symlink git-sfs manages.
pub trait Repo {
    /// Every candidate symlink at or below `scope`.
    /// `scope` is repo-relative (`.` for the whole repository) or absolute;
    /// either way it is resolved against the repository root.
    /// Results are sorted by path.
    ///
    /// Deduplication by hash, when a caller needs it for an object-level
    /// operation, is the caller's job — multiple paths legitimately share
    /// one hash, and different callers want that collapsed differently
    /// (`status`/`verify` report every path; `push`/`pull` only care about
    /// the unique object set).
    ///
    /// # Errors
    ///
    /// Returns [`RepoError::Walk`] if `scope` does not exist or a directory
    /// within it could not be read. Returns [`RepoError::Canceled`] if
    /// `cancel` fires.
    fn scan(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<ScannedEntry>, RepoError>;

    /// Every regular file at or below `scope` — `add`'s candidate set.
    /// `scope` is resolved exactly like [`Repo::scan`]'s. Results are sorted
    /// by path.
    ///
    /// # Errors
    ///
    /// Same as [`Repo::scan`], plus [`RepoError::Git`] if Git cannot answer
    /// whether candidate files are already tracked.
    fn find_files(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<FoundEntry>, RepoError>;
}

/// Excludes `.git` anywhere, the whole `.git-sfs` directory, and the top-level
/// `.gitignore`.
///
/// Operates on the repo-relative path. Checking whether any component equals
/// `.git-sfs` excludes the metadata directory and all of its descendants in one
/// place.
///
/// `pub(crate)`: [`super::super::exec::import`] reuses this for its own
/// "destination must not be inside `.git-sfs`" check rather than duplicating
/// the same component comparison a third time.
pub(crate) fn should_skip(repo_relative: &Utf8Path) -> bool {
    repo_relative.file_name() == Some(".git")
        || repo_relative
            .components()
            .any(|component| component.as_str() == ".git-sfs")
        || repo_relative == Utf8Path::new(".gitignore")
}

/// Resolves an absolute `scope` as-is (lexically cleaned), and a relative one
/// against `repo`.
///
/// `pub(crate)`: [`super::super::exec::mv`] resolves its `src`/`dest`
/// arguments the same way `Repo::scan`'s own `scope` does, so it reuses this
/// rather than duplicating the absolute-vs-relative branch a third time.
pub(crate) fn resolve_scope(repo: &Utf8Path, scope: &Utf8Path) -> Utf8PathBuf {
    if scope.is_absolute() {
        clean_utf8(scope)
    } else {
        clean_utf8(&repo.join(scope))
    }
}

/// `readlink()` plus [`validate_symlink_target`], the pair every real
/// candidate goes through — shared by [`FsRepo`] (reading a real symlink)
/// and [`FakeRepo`] (validating seeded target text), so the two
/// implementations cannot silently drift on what counts as valid.
fn read_and_validate(
    repo: &Utf8Path,
    file: &Utf8Path,
    target: &str,
) -> Result<Sha256, InvalidReason> {
    Ok(validate_symlink_target(repo, file, target)?)
}

/// The real, filesystem-backed [`Repo`].
pub struct FsRepo {
    repo: Utf8PathBuf,
}

impl FsRepo {
    /// A repo rooted at `repo` — the already-resolved repository root.
    #[must_use]
    pub fn new(repo: impl Into<Utf8PathBuf>) -> Self {
        Self { repo: repo.into() }
    }
}

impl Repo for FsRepo {
    fn scan(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<ScannedEntry>, RepoError> {
        let root = resolve_scope(&self.repo, scope);
        if is_symlink_scope(&self.repo, &root)? {
            return Ok(vec![scan_symlink(&self.repo, &root)]);
        }
        let mut entries = Vec::new();
        for item in filtered_walk(self.repo.clone(), &root) {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            let item = item?;
            if !item.file_type().is_symlink() {
                continue;
            }
            entries.push(match Utf8Path::from_path(item.path()) {
                Some(abs) => scan_symlink(&self.repo, abs),
                None => ScannedEntry::Unrepresentable {
                    description: lossy_relative(&self.repo, item.path()),
                },
            });
        }
        entries.sort_by(|a, b| a.path().cmp(&b.path()));
        Ok(entries)
    }

    fn find_files(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<FoundEntry>, RepoError> {
        let root = resolve_scope(&self.repo, scope);
        let tracked = git_tracked_files(&self.repo, &root)?;
        let mut entries = Vec::new();
        for item in filtered_walk(self.repo.clone(), &root) {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            let item = item?;
            if !item.file_type().is_file() {
                continue;
            }
            entries.push(match Utf8Path::from_path(item.path()) {
                Some(abs) => {
                    let path = abs.strip_prefix(&self.repo).unwrap_or(abs).to_owned();
                    FoundEntry::File {
                        git_tracked: tracked.contains(&path),
                        path,
                    }
                }
                None => FoundEntry::Unrepresentable {
                    description: lossy_relative(&self.repo, item.path()),
                },
            });
        }
        entries.sort_by(|a, b| a.path().cmp(&b.path()));
        Ok(entries)
    }
}

fn is_symlink_scope(repo: &Utf8Path, root: &Utf8Path) -> Result<bool, RepoError> {
    let relative = root.strip_prefix(repo).unwrap_or(root);
    if should_skip(relative) {
        return Ok(false);
    }
    match std::fs::symlink_metadata(root.as_std_path()) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Ok(false),
    }
}

fn scan_symlink(repo: &Utf8Path, abs: &Utf8Path) -> ScannedEntry {
    let path = abs.strip_prefix(repo).unwrap_or(abs).to_owned();
    match read_symlink_target(abs) {
        Ok(target) => match read_and_validate(repo, abs, &target) {
            Ok(hash) => ScannedEntry::Tracked { path, hash },
            Err(reason) => ScannedEntry::Invalid { path, reason },
        },
        Err(reason) => ScannedEntry::Invalid { path, reason },
    }
}

fn git_tracked_files(repo: &Utf8Path, root: &Utf8Path) -> Result<BTreeSet<Utf8PathBuf>, RepoError> {
    let Ok(scope) = root.strip_prefix(repo) else {
        return Ok(BTreeSet::new());
    };
    let pathspec = if scope.as_str().is_empty() {
        Utf8Path::new(".")
    } else {
        scope
    };

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.as_std_path())
        .args(["ls-files", "-z", "--"])
        .arg(pathspec.as_std_path())
        .output()
        .map_err(|err| RepoError::Git {
            repo: repo.to_owned(),
            detail: format!("run git: {err}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(RepoError::Git {
            repo: repo.to_owned(),
            detail: if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            },
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|err| RepoError::Git {
        repo: repo.to_owned(),
        detail: format!("git output was not valid UTF-8: {err}"),
    })?;
    Ok(stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(Utf8PathBuf::from)
        .collect())
}

/// A [`walkdir`] iterator over `root`, pruning subtrees [`should_skip`]
/// excludes. Shared by [`Repo::scan`] and [`Repo::find_files`]: both need
/// identical traversal/exclusion rules and differ only in which entry type
/// they keep and how they classify a match, so only this setup — the part
/// with the trickiest closure-capture semantics — is factored out, not the
/// two loop bodies themselves.
fn filtered_walk(
    repo: Utf8PathBuf,
    root: &Utf8Path,
) -> impl Iterator<Item = walkdir::Result<walkdir::DirEntry>> {
    walkdir::WalkDir::new(root.as_std_path())
        .into_iter()
        .filter_entry(move |entry| match Utf8Path::from_path(entry.path()) {
            Some(abs) => !should_skip(abs.strip_prefix(&repo).unwrap_or(abs)),
            // A non-UTF-8 path can't be classified by should_skip; erring on
            // the side of not pruning just means walking a bit more, never a
            // correctness problem (it still can't become a Tracked/File
            // entry once reached by the caller's own loop).
            None => true,
        })
}

/// `readlink()`, classifying the two ways it can fail to hand back usable
/// target text: the syscall itself erroring, or succeeding with bytes that
/// are not valid UTF-8.
fn read_symlink_target(path: &Utf8Path) -> Result<String, InvalidReason> {
    let target = std::fs::read_link(path).map_err(InvalidReason::Unreadable)?;
    target
        .into_os_string()
        .into_string()
        .map_err(|_| InvalidReason::TargetNotUtf8)
}

/// A best-effort, display-only rendering of `absolute` relative to `repo`,
/// for the one case a real repo-relative `Utf8PathBuf` cannot be produced —
/// `absolute` itself is not valid UTF-8. See
/// [`ScannedEntry::Unrepresentable`]/[`FoundEntry::Unrepresentable`].
fn lossy_relative(repo: &Utf8Path, absolute: &std::path::Path) -> String {
    absolute
        .strip_prefix(repo.as_std_path())
        .unwrap_or(absolute)
        .to_string_lossy()
        .into_owned()
}

/// An in-memory [`Repo`], for tests above this layer that need a repository
/// tree without a real filesystem.
///
/// Seeded entries are raw target *text*, exactly what `readlink()` would
/// return — not a pre-classified Tracked/Invalid — and [`FakeRepo::scan`]
/// runs them through the same [`read_and_validate`] real repos use. A fake
/// that let a test pre-declare an entry's classification could disagree with
/// real validation and let a higher-layer test pass against behavior no real
/// `Repo` would ever produce.
pub struct FakeRepo {
    repo: Utf8PathBuf,
    links: Mutex<BTreeMap<Utf8PathBuf, String>>,
    files: Mutex<BTreeSet<Utf8PathBuf>>,
    tracked_files: Mutex<BTreeSet<Utf8PathBuf>>,
}

impl FakeRepo {
    /// A repo rooted at `repo`, with nothing seeded yet.
    #[must_use]
    pub fn new(repo: impl Into<Utf8PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            links: Mutex::default(),
            files: Mutex::default(),
            tracked_files: Mutex::default(),
        }
    }

    /// Seeds a symlink at repo-relative `path` with raw target text. An
    /// intentionally-invalid target (absolute, outside the cache, wrong
    /// hash) is a legitimate thing to seed here — it exercises the same
    /// rejection path a real broken link would.
    pub fn seed(&self, path: impl Into<Utf8PathBuf>, raw_target: impl Into<String>) {
        self.links
            .lock()
            .expect("fake repo mutex poisoned")
            .insert(path.into(), raw_target.into());
    }

    /// Seeds a regular file at repo-relative `path`, for
    /// [`Repo::find_files`] tests. No content: `find_files` only reports
    /// paths, never reads bytes -- an `add` orchestration test still needs a
    /// real file on disk for the hashing step regardless of which `Repo` it
    /// uses, so this fake's job is only to exercise selection/scoping, not
    /// to avoid the filesystem entirely.
    pub fn seed_file(&self, path: impl Into<Utf8PathBuf>) {
        self.files
            .lock()
            .expect("fake repo mutex poisoned")
            .insert(path.into());
    }

    /// Marks a seeded regular file as already tracked by Git.
    pub fn seed_git_tracked_file(&self, path: impl Into<Utf8PathBuf>) {
        self.tracked_files
            .lock()
            .expect("fake repo mutex poisoned")
            .insert(path.into());
    }
}

/// `scope`, resolved and expressed relative to `repo` — the form
/// [`FakeRepo::scan`]/[`FakeRepo::find_files`] both filter seeded
/// repo-relative paths against.
fn fake_scope_rel(repo: &Utf8Path, scope: &Utf8Path) -> Utf8PathBuf {
    let resolved = resolve_scope(repo, scope);
    resolved
        .strip_prefix(repo)
        .map(Utf8Path::to_owned)
        .unwrap_or_else(|_| resolved.clone())
}

/// Whether repo-relative `path` falls within `scope_rel`; `.` means everything.
fn in_scope(scope_rel: &Utf8Path, path: &Utf8Path) -> bool {
    scope_rel == Utf8Path::new(".") || path.starts_with(scope_rel)
}

impl Repo for FakeRepo {
    fn scan(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<ScannedEntry>, RepoError> {
        let scope_rel = fake_scope_rel(&self.repo, scope);

        let links = self.links.lock().expect("fake repo mutex poisoned");
        let mut entries = Vec::new();
        for (path, target) in links.iter() {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            if !in_scope(&scope_rel, path) || should_skip(path) {
                continue;
            }
            let abs_path = self.repo.join(path);
            entries.push(match read_and_validate(&self.repo, &abs_path, target) {
                Ok(hash) => ScannedEntry::Tracked {
                    path: path.clone(),
                    hash,
                },
                Err(reason) => ScannedEntry::Invalid {
                    path: path.clone(),
                    reason,
                },
            });
        }
        // BTreeMap iteration is already sorted by key (path).
        Ok(entries)
    }

    fn find_files(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<FoundEntry>, RepoError> {
        let scope_rel = fake_scope_rel(&self.repo, scope);

        let files = self.files.lock().expect("fake repo mutex poisoned");
        let tracked_files = self.tracked_files.lock().expect("fake repo mutex poisoned");
        let mut entries = Vec::new();
        for path in files.iter() {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            if !in_scope(&scope_rel, path) || should_skip(path) {
                continue;
            }
            entries.push(FoundEntry::File {
                git_tracked: tracked_files.contains(path),
                path: path.clone(),
            });
        }
        // BTreeSet iteration is already sorted.
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git-sfs/cache/files/sha256/ab")).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["init", "--quiet"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        repo
    }

    fn utf8(dir: &tempfile::TempDir) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap()
    }

    /// Writes a valid git-sfs symlink at `repo/rel_path` for `hash`, the way
    /// `add` would.
    fn link_valid(repo: &Utf8Path, rel_path: &str, hash: Sha256) {
        let file = repo.join(rel_path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let target = crate::domain::symlink::git_link_target(repo, &file, hash).unwrap();
        std::os::unix::fs::symlink(target.as_std_path(), file.as_std_path()).unwrap();
    }

    fn a_hash() -> Sha256 {
        Sha256::parse("ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a").unwrap()
    }

    fn another_hash() -> Sha256 {
        Sha256::parse(&format!("cd{}cd", "91".repeat(30))).unwrap()
    }

    #[test]
    fn scan_finds_a_valid_tracked_symlink() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "data/train.bin", a_hash());

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ScannedEntry::Tracked { path, hash } => {
                assert_eq!(path, "data/train.bin");
                assert_eq!(*hash, a_hash());
            }
            other => panic!("expected Tracked, got {other:?}"),
        }
    }

    #[test]
    fn scan_classifies_a_dangling_symlink_scope() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "data/train.bin", a_hash());

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("data/train.bin"), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ScannedEntry::Tracked { path, hash } => {
                assert_eq!(path, "data/train.bin");
                assert_eq!(*hash, a_hash());
            }
            other => panic!("expected Tracked, got {other:?}"),
        }
    }

    #[test]
    fn scan_reports_an_invalid_symlink_without_aborting() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "good.bin", a_hash());
        // A symlink pointing somewhere absolute -- rule 2's violation.
        std::os::unix::fs::symlink("/etc/passwd", repo.join("bad.bin").as_std_path()).unwrap();

        let mut entries = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &Cancel::new())
            .unwrap();
        entries.sort_by_key(|e| e.path().map(Utf8Path::to_owned));

        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], ScannedEntry::Invalid { .. }));
        assert!(matches!(entries[1], ScannedEntry::Tracked { .. }));
    }

    #[test]
    fn scan_excludes_git_and_git_sfs_but_still_finds_real_links() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "keep.bin", a_hash());
        // Neither of these should ever surface, valid-looking or not.
        std::os::unix::fs::symlink("/etc/passwd", repo.join(".git/inside.bin").as_std_path())
            .unwrap();
        std::fs::create_dir_all(repo.join("nested/.git-sfs")).unwrap();
        std::os::unix::fs::symlink(
            "/etc/passwd",
            repo.join("nested/.git-sfs/inside.bin").as_std_path(),
        )
        .unwrap();

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("keep.bin")));
    }

    #[test]
    fn scan_scopes_to_the_requested_subtree() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "keep/a.bin", a_hash());
        link_valid(&repo, "elsewhere/b.bin", another_hash());

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("keep"), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("keep/a.bin")));
    }

    #[test]
    fn scan_results_are_sorted_by_path() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "z.bin", a_hash());
        link_valid(&repo, "a.bin", another_hash());
        link_valid(&repo, "m.bin", a_hash());

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &Cancel::new())
            .unwrap();
        let paths: Vec<&Utf8Path> = entries.iter().map(|e| e.path().unwrap()).collect();
        assert_eq!(paths, vec!["a.bin", "m.bin", "z.bin"]);
    }

    #[test]
    fn scan_errors_when_the_scope_does_not_exist() {
        let dir = init_repo();
        let repo = utf8(&dir);
        let err = FsRepo::new(repo)
            .scan(Utf8Path::new("does/not/exist"), &Cancel::new())
            .unwrap_err();
        assert!(matches!(err, RepoError::Walk(_)));
    }

    #[test]
    fn scan_stops_promptly_when_already_canceled() {
        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "a.bin", a_hash());
        let cancel = Cancel::new();
        cancel.cancel();

        let err = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &cancel)
            .unwrap_err();
        assert!(matches!(err, RepoError::Canceled));
    }

    /// A symlink whose own name is not valid UTF-8 is skipped (not
    /// validated) and reported (not silently dropped), and the rest of the
    /// scan proceeds -- it must never abort the whole operation over one
    /// unreadable name.
    ///
    /// Linux-only: this scenario is unreachable on Darwin, where APFS
    /// enforces valid UTF-8 in filenames at the syscall level and
    /// `symlink()` itself fails with `EILSEQ` before a test could ever
    /// construct the file this test needs.
    #[cfg(target_os = "linux")]
    #[test]
    fn scan_skips_and_reports_a_non_utf8_named_symlink_without_aborting() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = init_repo();
        let repo = utf8(&dir);
        link_valid(&repo, "good.bin", a_hash());

        // 0x80 alone is not valid UTF-8 in any position.
        let bad_name = OsStr::from_bytes(b"bad-\xFF-name.bin");
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join(bad_name)).unwrap();

        let entries = FsRepo::new(repo)
            .scan(Utf8Path::new("."), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, ScannedEntry::Unrepresentable { .. }))
        );
        assert!(entries.iter().any(|e| matches!(
            e,
            ScannedEntry::Tracked { path, .. } if path == "good.bin"
        )));
    }

    #[test]
    fn fake_repo_validates_seeded_targets_the_same_way_a_real_repo_would() {
        let repo = Utf8PathBuf::from("/repo");
        let fake = FakeRepo::new(repo.clone());
        let valid_target =
            crate::domain::symlink::git_link_target(&repo, &repo.join("data/train.bin"), a_hash())
                .unwrap();
        fake.seed("data/train.bin", valid_target.to_string());
        fake.seed("data/broken.bin", "/etc/passwd"); // absolute -- rule 2

        let mut entries = fake.scan(Utf8Path::new("."), &Cancel::new()).unwrap();
        entries.sort_by_key(|e| e.path().map(Utf8Path::to_owned));

        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], ScannedEntry::Invalid { .. }));
        match &entries[1] {
            ScannedEntry::Tracked { path, hash } => {
                assert_eq!(path, "data/train.bin");
                assert_eq!(*hash, a_hash());
            }
            other => panic!("expected Tracked, got {other:?}"),
        }
    }

    #[test]
    fn fake_repo_respects_scope_and_skip_rules() {
        let repo = Utf8PathBuf::from("/repo");
        let fake = FakeRepo::new(repo.clone());
        let target =
            crate::domain::symlink::git_link_target(&repo, &repo.join("keep/a.bin"), a_hash())
                .unwrap();
        fake.seed("keep/a.bin", target.to_string());
        let elsewhere_target = crate::domain::symlink::git_link_target(
            &repo,
            &repo.join("elsewhere/b.bin"),
            another_hash(),
        )
        .unwrap();
        fake.seed("elsewhere/b.bin", elsewhere_target.to_string());
        fake.seed(".git-sfs/rogue", "/etc/passwd");

        let entries = fake.scan(Utf8Path::new("keep"), &Cancel::new()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("keep/a.bin")));
    }

    #[test]
    fn find_files_finds_regular_files_but_not_symlinks() {
        let dir = init_repo();
        let repo = utf8(&dir);
        std::fs::write(repo.join("plain.bin"), b"regular file bytes").unwrap();
        link_valid(&repo, "linked.bin", a_hash());

        let entries = FsRepo::new(repo)
            .find_files(Utf8Path::new("."), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("plain.bin")));
    }

    #[test]
    fn find_files_marks_files_already_tracked_by_git() {
        let dir = init_repo();
        let repo = utf8(&dir);
        std::fs::write(repo.join("README.md"), b"tracked").unwrap();
        std::fs::write(repo.join("data.bin"), b"untracked").unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.as_std_path())
            .args(["add", "README.md"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let entries = FsRepo::new(repo)
            .find_files(Utf8Path::new("."), &Cancel::new())
            .unwrap();

        assert!(entries.iter().any(|entry| matches!(
            entry,
            FoundEntry::File { path, git_tracked: true } if path == "README.md"
        )));
        assert!(entries.iter().any(|entry| matches!(
            entry,
            FoundEntry::File { path, git_tracked: false } if path == "data.bin"
        )));
    }

    #[test]
    fn find_files_excludes_git_and_git_sfs_and_respects_scope() {
        let dir = init_repo();
        let repo = utf8(&dir);
        std::fs::create_dir_all(repo.join("keep")).unwrap();
        std::fs::write(repo.join("keep/a.bin"), b"a").unwrap();
        std::fs::create_dir_all(repo.join("elsewhere")).unwrap();
        std::fs::write(repo.join("elsewhere/b.bin"), b"b").unwrap();
        std::fs::write(repo.join(".git/inside.bin"), b"never").unwrap();

        let entries = FsRepo::new(repo)
            .find_files(Utf8Path::new("keep"), &Cancel::new())
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("keep/a.bin")));
    }

    #[test]
    fn fake_repo_find_files_respects_scope_and_skip_rules() {
        let repo = Utf8PathBuf::from("/repo");
        let fake = FakeRepo::new(repo);
        fake.seed_file("keep/a.bin");
        fake.seed_file("elsewhere/b.bin");
        fake.seed_file(".git-sfs/rogue.bin");
        fake.seed_git_tracked_file("keep/a.bin");

        let entries = fake
            .find_files(Utf8Path::new("keep"), &Cancel::new())
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Some(Utf8Path::new("keep/a.bin")));
        assert!(matches!(
            entries[0],
            FoundEntry::File {
                git_tracked: true,
                ..
            }
        ));
    }
}
