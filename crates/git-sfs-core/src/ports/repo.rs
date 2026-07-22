//! The repository: walking the tree, reading the symlinks on it.
//!
//! contract-spec §3.3/§5b, rust-rewrite-plan §3.3. [`Repo`] gets a trait
//! because there are genuinely two implementations — [`FsRepo`] (real) and
//! [`FakeRepo`] (in-memory, for higher-layer tests) — the bar
//! rust-rewrite-plan §3.3 sets for introducing one at all.
//!
//! [`Repo::scan`] is one shared mechanism standing in for v1's *two* walks
//! that do the same traversal for different purposes: `collectGitSFSSymlinks`
//! (`walk.go:18`, used by `push`/`pull`/`status`/`init`), which silently
//! drops anything that fails symlink validation, and `verify`'s own walk
//! (`verify.go:120-155`), which instead reports each failure as a "broken
//! git symlink" issue. Returning every candidate here — [`ScannedEntry::Tracked`]
//! for one that validates, [`ScannedEntry::Invalid`] for one that doesn't —
//! unifies the mechanism; which policy a given command applies (drop the
//! invalid ones, or report them) is a Phase 4 decision, not this port's.
//! `git-sfs-core` cannot print (see the crate doc), so even the "report" side
//! of that stays data here — a command layer turns it into a warning.
//!
//! Directory-read failures (or the requested scope not existing at all)
//! abort the whole scan with `Err`, matching v1's `filepath.WalkDir` and
//! rust-rewrite-plan §2.5's "cannot determine the tree's contents" being an
//! error, not a silently partial result. A single candidate symlink failing
//! validation does not abort — matching v1's per-entry `return nil`.

use std::collections::BTreeMap;
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
    /// A valid git-sfs symlink — contract-spec §3.2's six rules all held.
    Tracked {
        /// Repo-relative path to the symlink.
        path: Utf8PathBuf,
        /// The hash its target names.
        hash: Sha256,
    },
    /// A symlink that looked like a candidate but is not a valid git-sfs
    /// link — either unrelated to git-sfs, or a broken/hand-edited one.
    /// `push`/`pull`/`status` silently ignore these, mirroring v1's
    /// `collectGitSFSSymlinks`; `verify` is expected to report them, matching
    /// v1's own separate walk (`verify.go:143-151`, "broken git symlink").
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

/// Why [`Repo::scan`] could not validate a candidate symlink as a git-sfs
/// link.
#[derive(Debug, Error)]
pub enum InvalidReason {
    /// `readlink()` itself failed — e.g. permission denied, or the entry
    /// disappeared between being listed and being read. Treated as
    /// unresolvable, the same as v1's `ParseGitSymlink` does when
    /// `os.Readlink` errors.
    #[error("reading symlink: {0}")]
    Unreadable(#[source] std::io::Error),
    /// The target text `readlink()` returned is not valid UTF-8 —
    /// unrepresentable as the `&str` contract-spec §3.2's validation rules
    /// operate over. Distinct from [`ScannedEntry::Unrepresentable`], which
    /// is about the symlink's own *name*, not what it points at.
    #[error("symlink target is not valid UTF-8")]
    TargetNotUtf8,
    /// One of contract-spec §3.2's six validation rules failed.
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
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<RepoError> for Error {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::Walk(_) => Error::Unavailable(err.to_string()),
            RepoError::Canceled => Error::Canceled,
        }
    }
}

/// A repository tree: the source of every symlink git-sfs manages.
pub trait Repo {
    /// Every candidate symlink at or below `scope` (contract-spec §5b).
    /// `scope` is repo-relative (`.` for the whole repository) or absolute;
    /// either way it is resolved the same way v1's `absFromRepo` does.
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
}

/// v1's `shouldSkip` (`walk.go:58-65`), ported: exclude `.git` anywhere, the
/// whole `.git-sfs` directory (cache, config, locks, tmp — however deep it's
/// nested), and the top-level `.gitignore`.
///
/// Operates on the repo-relative path rather than v1's absolute-path string
/// comparisons. v1 spends three separate checks on `.git-sfs` (the exact
/// top-level directory, its `config.toml` child, and a substring probe
/// `strings.Contains(path, "/.git-sfs/")` for deeper nesting); "does any path
/// component equal `.git-sfs`" is one check that produces the identical
/// excluded set — a directory-vs-its-contents nuance in exactly *when* each
/// approach prunes does not change *what* ends up in the final symlink list,
/// since directories are never candidates either way. See the port's test
/// suite for cases exercising this directly.
fn should_skip(repo_relative: &Utf8Path) -> bool {
    repo_relative.file_name() == Some(".git")
        || repo_relative
            .components()
            .any(|component| component.as_str() == ".git-sfs")
        || repo_relative == Utf8Path::new(".gitignore")
}

/// v1's `absFromRepo` (`walk.go:80-85`): an absolute `scope` is used as-is
/// (lexically cleaned), a relative one is resolved against `repo`.
fn resolve_scope(repo: &Utf8Path, scope: &Utf8Path) -> Utf8PathBuf {
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
        let repo_for_filter = self.repo.clone();
        let walker = walkdir::WalkDir::new(root.as_std_path())
            .into_iter()
            .filter_entry(move |entry| match Utf8Path::from_path(entry.path()) {
                Some(abs) => !should_skip(abs.strip_prefix(&repo_for_filter).unwrap_or(abs)),
                // A non-UTF-8 path can't be classified by should_skip; erring
                // on the side of not pruning just means walking a bit more,
                // never a correctness problem (it still can't become a
                // Tracked entry once reached -- see below).
                None => true,
            });

        let mut entries = Vec::new();
        for item in walker {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            let item = item?;
            if !item.file_type().is_symlink() {
                continue;
            }
            match Utf8Path::from_path(item.path()) {
                Some(abs) => {
                    let path = abs.strip_prefix(&self.repo).unwrap_or(abs).to_owned();
                    entries.push(match read_symlink_target(abs) {
                        Ok(target) => match read_and_validate(&self.repo, abs, &target) {
                            Ok(hash) => ScannedEntry::Tracked { path, hash },
                            Err(reason) => ScannedEntry::Invalid { path, reason },
                        },
                        Err(reason) => ScannedEntry::Invalid { path, reason },
                    });
                }
                None => {
                    let relative = item
                        .path()
                        .strip_prefix(self.repo.as_std_path())
                        .unwrap_or(item.path());
                    entries.push(ScannedEntry::Unrepresentable {
                        description: relative.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        entries.sort_by(|a, b| a.path().cmp(&b.path()));
        Ok(entries)
    }
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

/// An in-memory [`Repo`], for tests above this layer that need a repository
/// tree without a real filesystem. Its existence is what justifies `Repo`
/// being a trait at all (rust-rewrite-plan §3.3).
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
}

impl FakeRepo {
    /// A repo rooted at `repo`, with no symlinks seeded yet.
    #[must_use]
    pub fn new(repo: impl Into<Utf8PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            links: Mutex::default(),
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
}

impl Repo for FakeRepo {
    fn scan(&self, scope: &Utf8Path, cancel: &Cancel) -> Result<Vec<ScannedEntry>, RepoError> {
        let resolved = resolve_scope(&self.repo, scope);
        let scope_rel = resolved
            .strip_prefix(&self.repo)
            .map(Utf8Path::to_owned)
            .unwrap_or_else(|_| resolved.clone());

        let links = self.links.lock().expect("fake repo mutex poisoned");
        let mut entries = Vec::new();
        for (path, target) in links.iter() {
            if cancel.is_canceled() {
                return Err(RepoError::Canceled);
            }
            if scope_rel != Utf8Path::new(".") && !path.starts_with(&scope_rel) {
                continue;
            }
            if should_skip(path) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git-sfs/cache/files/sha256/ab")).unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
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
}
