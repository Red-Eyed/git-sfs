//! `mv` — ported from `mv.go`. Moves a git-sfs symlink, or a whole directory
//! of them, to a new location and rewrites each relative target for its new
//! depth from the repository root.
//!
//! Deliberately never touches the cache: contract-spec §3.3 is explicit that
//! a committed symlink is the unit of operation, not the object behind it,
//! and `mv` must succeed even when that object is absent — reorganizing a
//! dataset before a `pull` has finished is exactly the recovery case this
//! exists for.
//!
//! Both branches below mirror v1's `mvLink`/`mvDir` (`mv.go:35-117`)
//! exactly, including two of its sequencing choices that matter for
//! correctness, not just style:
//!
//! - The single-symlink case validates `src` as a git-sfs symlink and writes
//!   the new symlink *before* removing the old one, then rolls the new one
//!   back if removing the old one fails — never leaving neither in place.
//! - The directory case collects every tracked symlink *before* renaming
//!   anything (their relative targets can only be validated against their
//!   current location), performs the whole move as one atomic `rename()`,
//!   and only then rewrites each relocated symlink's target for its new
//!   location.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::domain::symlink::{NoRelativePath, git_link_target, validate_symlink_target};
use crate::error::Error;
use crate::ports::repo::resolve_scope;
use crate::ports::{Repo, RepoError, ScannedEntry};

use super::repo_relative;

/// One symlink `mv` relocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedLink {
    /// Repo-relative path before the move.
    pub old_path: Utf8PathBuf,
    /// Repo-relative path after the move.
    pub new_path: Utf8PathBuf,
}

/// Why `mv` failed.
#[derive(Debug, Error)]
pub enum MvError {
    /// `src` exists but is not a valid git-sfs symlink — either not a
    /// symlink at all, its target is not valid UTF-8, or the target fails
    /// contract-spec §3.2's validation. Mirrors v1's undifferentiated wrap
    /// (`mv.go:38`): by the time this is checked, `mv` has already confirmed
    /// the path exists, so every remaining failure mode means the same
    /// thing to the caller — this is not something `mv` can operate on.
    #[error("{path} is not a git-sfs symlink")]
    NotATrackedLink {
        /// The path that failed to validate.
        path: Utf8PathBuf,
    },
    /// `dst` (after POSIX "place inside an existing directory" resolution)
    /// already exists.
    #[error("destination already exists: {path}")]
    DestinationExists {
        /// The path that was already occupied.
        path: Utf8PathBuf,
    },
    /// `dst` and the repository root disagree on absolute-vs-relative, so no
    /// symlink target could be computed for it. Not reachable in practice:
    /// both are always absolute by the time they reach here.
    #[error("{path}: {source}")]
    NoRelativePath {
        /// The destination path being computed for.
        path: Utf8PathBuf,
        /// The underlying error.
        #[source]
        source: NoRelativePath,
    },
    /// Scanning a source directory for tracked symlinks failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// A filesystem operation (stat, rename, symlink, remove) failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<MvError> for Error {
    fn from(err: MvError) -> Self {
        match &err {
            // Cancellation outranks every other classification (see
            // Error::Canceled's doc) whether it surfaced through the
            // directory-scan's own RepoError or mv's own retargeting loop.
            MvError::Repo(RepoError::Canceled) | MvError::Canceled => Error::Canceled,
            MvError::NotATrackedLink { .. } | MvError::DestinationExists { .. } => {
                Error::Usage(err.to_string())
            }
            MvError::NoRelativePath { .. } => Error::Config(err.to_string()),
            MvError::Repo(_) | MvError::Io { .. } => Error::Unavailable(err.to_string()),
        }
    }
}

/// [`mv`]'s outcome-so-far, together with why it stopped — returned instead
/// of a bare [`MvError`] so a caller can still report links already moved
/// when the directory case fails partway through retargeting.
#[derive(Debug)]
pub struct MvFailure {
    /// Every link successfully relocated before `error` stopped the run.
    pub moved: Vec<MovedLink>,
    /// Why it stopped. Boxed for the same reason as `AddFailure::error`:
    /// keeps the success path from carrying the error type's full size.
    pub error: Box<MvError>,
}

impl MvFailure {
    fn new(moved: Vec<MovedLink>, error: MvError) -> Self {
        Self {
            moved,
            error: Box::new(error),
        }
    }
}

/// Moves `src` to `dst`, both resolved the same way [`Repo::scan`]'s own
/// scope argument is (an absolute path is used as-is, a relative one
/// resolves against `repo` — v1's `absFromRepo`).
///
/// `src`'s own [`std::fs::symlink_metadata`] decides which of v1's two
/// branches applies: a directory moves as a tree, anything else moves as a
/// single symlink.
///
/// # Errors
///
/// Returns the links moved so far bundled with the first [`MvError`]
/// encountered, so a caller can report partial progress even on failure.
pub fn mv(
    repo_port: &dyn Repo,
    repo: &Utf8Path,
    src: &Utf8Path,
    dst: &Utf8Path,
    cancel: &Cancel,
) -> Result<Vec<MovedLink>, MvFailure> {
    let src_abs = resolve_scope(repo, src);
    let dst_abs = resolve_scope(repo, dst);

    let metadata = std::fs::symlink_metadata(src_abs.as_std_path()).map_err(|source| {
        MvFailure::new(
            Vec::new(),
            MvError::Io {
                path: src_abs.clone(),
                source,
            },
        )
    })?;

    if metadata.is_dir() {
        mv_dir(repo_port, repo, &src_abs, &dst_abs, cancel)
    } else {
        mv_file(repo, &src_abs, &dst_abs)
            .map(|moved| vec![moved])
            .map_err(|error| MvFailure::new(Vec::new(), error))
    }
}

/// `readlink()` at `path`, validated as a git-sfs symlink target -- v1's
/// `ParseGitSymlink` (`mv.go:36`).
fn read_tracked_link(repo: &Utf8Path, path: &Utf8Path) -> Result<Sha256, MvError> {
    let raw = std::fs::read_link(path.as_std_path()).map_err(|_| MvError::NotATrackedLink {
        path: path.to_owned(),
    })?;
    let target = raw
        .into_os_string()
        .into_string()
        .map_err(|_| MvError::NotATrackedLink {
            path: path.to_owned(),
        })?;
    validate_symlink_target(repo, path, &target).map_err(|_| MvError::NotATrackedLink {
        path: path.to_owned(),
    })
}

/// If `dst_abs` is an existing directory, POSIX `mv` places the source
/// inside it under its own basename rather than replacing the directory.
fn place_inside_existing_dir(dst_abs: &Utf8Path, basename: &Utf8Path) -> Utf8PathBuf {
    if std::fs::symlink_metadata(dst_abs.as_std_path()).is_ok_and(|metadata| metadata.is_dir()) {
        dst_abs.join(basename)
    } else {
        dst_abs.to_owned()
    }
}

/// Moves a single git-sfs symlink from `src_abs` to `dst_abs` -- v1's
/// `mvLink` (`mv.go:35-63`).
fn mv_file(repo: &Utf8Path, src_abs: &Utf8Path, dst_abs: &Utf8Path) -> Result<MovedLink, MvError> {
    let hash = read_tracked_link(repo, src_abs)?;

    let basename = src_abs
        .file_name()
        .expect("a symlink path always has a final component");
    let dst_abs = place_inside_existing_dir(dst_abs, Utf8Path::new(basename));

    if std::fs::symlink_metadata(dst_abs.as_std_path()).is_ok() {
        return Err(MvError::DestinationExists { path: dst_abs });
    }
    if let Some(parent) = dst_abs.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| MvError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }

    let target =
        git_link_target(repo, &dst_abs, hash).map_err(|source| MvError::NoRelativePath {
            path: dst_abs.clone(),
            source,
        })?;
    std::os::unix::fs::symlink(target.as_std_path(), dst_abs.as_std_path()).map_err(|source| {
        MvError::Io {
            path: dst_abs.clone(),
            source,
        }
    })?;

    if let Err(source) = std::fs::remove_file(src_abs.as_std_path()) {
        #[allow(
            clippy::let_underscore_must_use,
            reason = "best-effort rollback of the symlink just written above; whether this second remove succeeds or not, the original remove_file error below is what gets reported"
        )]
        let _ = std::fs::remove_file(dst_abs.as_std_path());
        return Err(MvError::Io {
            path: src_abs.to_owned(),
            source,
        });
    }

    Ok(MovedLink {
        old_path: repo_relative(repo, src_abs),
        new_path: repo_relative(repo, &dst_abs),
    })
}

/// Moves a directory of git-sfs symlinks from `src_abs` to `dst_abs` -- v1's
/// `mvDir` (`mv.go:65-117`).
fn mv_dir(
    repo_port: &dyn Repo,
    repo: &Utf8Path,
    src_abs: &Utf8Path,
    dst_abs: &Utf8Path,
    cancel: &Cancel,
) -> Result<Vec<MovedLink>, MvFailure> {
    let basename = src_abs
        .file_name()
        .expect("a directory path always has a final component");
    let dst_abs = place_inside_existing_dir(dst_abs, Utf8Path::new(basename));

    // Tracked links must be found and validated against src_abs's *current*
    // location -- a relative target only resolves correctly there.
    let links: Vec<(Utf8PathBuf, Sha256)> = repo_port
        .scan(src_abs, cancel)
        .map_err(|err| MvFailure::new(Vec::new(), MvError::Repo(err)))?
        .into_iter()
        .filter_map(|entry| match entry {
            ScannedEntry::Tracked { path, hash } => Some((path, hash)),
            ScannedEntry::Invalid { .. } | ScannedEntry::Unrepresentable { .. } => None,
        })
        .collect();

    if let Some(parent) = dst_abs.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| {
            MvFailure::new(
                Vec::new(),
                MvError::Io {
                    path: parent.to_owned(),
                    source,
                },
            )
        })?;
    }
    std::fs::rename(src_abs.as_std_path(), dst_abs.as_std_path()).map_err(|source| {
        MvFailure::new(
            Vec::new(),
            MvError::Io {
                path: src_abs.to_owned(),
                source,
            },
        )
    })?;

    let src_rel = repo_relative(repo, src_abs);
    let mut moved = Vec::new();
    for (old_path, hash) in links {
        if cancel.is_canceled() {
            return Err(MvFailure::new(moved, MvError::Canceled));
        }

        let within_dir = old_path.strip_prefix(&src_rel).unwrap_or(&old_path);
        let new_abs = dst_abs.join(within_dir);

        let target = match git_link_target(repo, &new_abs, hash) {
            Ok(target) => target,
            Err(source) => {
                return Err(MvFailure::new(
                    moved,
                    MvError::NoRelativePath {
                        path: new_abs,
                        source,
                    },
                ));
            }
        };
        // The rename already carried the stale-target symlink to new_abs;
        // replace it with one holding the correct target for this depth.
        if let Err(source) = std::fs::remove_file(new_abs.as_std_path()) {
            return Err(MvFailure::new(
                moved,
                MvError::Io {
                    path: new_abs,
                    source,
                },
            ));
        }
        if let Err(source) = std::os::unix::fs::symlink(target.as_std_path(), new_abs.as_std_path())
        {
            return Err(MvFailure::new(
                moved,
                MvError::Io {
                    path: new_abs,
                    source,
                },
            ));
        }

        moved.push(MovedLink {
            old_path,
            new_path: repo_relative(repo, &new_abs),
        });
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::FsRepo;

    fn init_repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs/cache/files/sha256")).unwrap();
        (dir, repo)
    }

    fn a_hash() -> Sha256 {
        Sha256::parse("ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a").unwrap()
    }

    /// Writes a valid git-sfs symlink at `repo/rel_path` for `hash`, the way
    /// `add` would.
    fn link_valid(repo: &Utf8Path, rel_path: &str, hash: Sha256) {
        let file = repo.join(rel_path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let target = git_link_target(repo, &file, hash).unwrap();
        std::os::unix::fs::symlink(target.as_std_path(), file.as_std_path()).unwrap();
    }

    #[test]
    fn moves_a_single_tracked_symlink_and_rewrites_its_target() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "data/a.bin", a_hash());
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let moved = mv(
            &repo_port,
            &repo,
            Utf8Path::new("data/a.bin"),
            Utf8Path::new("data/b.bin"),
            &cancel,
        )
        .unwrap();

        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].old_path, "data/a.bin");
        assert_eq!(moved[0].new_path, "data/b.bin");
        assert!(!repo.join("data/a.bin").exists());
        let target = std::fs::read_link(repo.join("data/b.bin")).unwrap();
        assert_eq!(
            validate_symlink_target(&repo, &repo.join("data/b.bin"), target.to_str().unwrap())
                .unwrap(),
            a_hash()
        );
    }

    #[test]
    fn moving_into_a_different_depth_rewrites_the_relative_target() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "a.bin", a_hash());
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        mv(
            &repo_port,
            &repo,
            Utf8Path::new("a.bin"),
            Utf8Path::new("nested/deep/b.bin"),
            &cancel,
        )
        .unwrap();

        let target = std::fs::read_link(repo.join("nested/deep/b.bin")).unwrap();
        let target = Utf8Path::new(target.to_str().unwrap());
        // Two directories deep now, so the climb back up to .git-sfs is one
        // segment longer than a_hash's own link at the repo root ("../").
        assert!(target.as_str().starts_with("../../.git-sfs/"));
    }

    #[test]
    fn placing_a_file_inside_an_existing_directory_uses_its_basename() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "a.bin", a_hash());
        std::fs::create_dir_all(repo.join("dest")).unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let moved = mv(
            &repo_port,
            &repo,
            Utf8Path::new("a.bin"),
            Utf8Path::new("dest"),
            &cancel,
        )
        .unwrap();

        assert_eq!(moved[0].new_path, "dest/a.bin");
    }

    #[test]
    fn refuses_to_move_a_plain_file_that_is_not_a_git_sfs_symlink() {
        let (_dir, repo) = init_repo();
        std::fs::write(repo.join("plain.bin"), b"not a link").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let failure = mv(
            &repo_port,
            &repo,
            Utf8Path::new("plain.bin"),
            Utf8Path::new("dst.bin"),
            &cancel,
        )
        .unwrap_err();

        assert!(failure.moved.is_empty());
        assert!(matches!(*failure.error, MvError::NotATrackedLink { .. }));
        assert!(repo.join("plain.bin").exists(), "source must be untouched");
    }

    #[test]
    fn refuses_an_existing_destination() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "a.bin", a_hash());
        std::fs::write(repo.join("b.bin"), b"already here").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let failure = mv(
            &repo_port,
            &repo,
            Utf8Path::new("a.bin"),
            Utf8Path::new("b.bin"),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(*failure.error, MvError::DestinationExists { .. }));
        // `.exists()` follows symlinks and would report false here since
        // a_hash's cache object was never actually written to disk in this
        // test -- checking for the symlink itself is what "untouched" means.
        assert!(
            std::fs::symlink_metadata(repo.join("a.bin")).is_ok(),
            "source must be untouched"
        );
    }

    #[test]
    fn moves_a_directory_of_tracked_symlinks_and_rewrites_every_target() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "data/a.bin", a_hash());
        link_valid(&repo, "data/nested/b.bin", a_hash());
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let mut moved = mv(
            &repo_port,
            &repo,
            Utf8Path::new("data"),
            Utf8Path::new("moved"),
            &cancel,
        )
        .unwrap();
        moved.sort_by(|a, b| a.new_path.cmp(&b.new_path));

        assert_eq!(moved.len(), 2);
        assert_eq!(moved[0].old_path, "data/a.bin");
        assert_eq!(moved[0].new_path, "moved/a.bin");
        assert_eq!(moved[1].old_path, "data/nested/b.bin");
        assert_eq!(moved[1].new_path, "moved/nested/b.bin");
        assert!(!repo.join("data").exists());

        for rel in ["moved/a.bin", "moved/nested/b.bin"] {
            let file = repo.join(rel);
            let target = std::fs::read_link(&file).unwrap();
            assert_eq!(
                validate_symlink_target(&repo, &file, target.to_str().unwrap()).unwrap(),
                a_hash()
            );
        }
    }

    #[test]
    fn moving_a_directory_ignores_symlinks_that_are_not_valid_git_sfs_links() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "data/a.bin", a_hash());
        std::os::unix::fs::symlink("/etc/passwd", repo.join("data/rogue.bin")).unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let moved = mv(
            &repo_port,
            &repo,
            Utf8Path::new("data"),
            Utf8Path::new("moved"),
            &cancel,
        )
        .unwrap();

        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].old_path, "data/a.bin");
        // The untracked symlink still rides along with the directory rename
        // -- mv only rewrites tracked links, it does not drop anything.
        assert!(repo.join("moved/rogue.bin").exists());
    }

    #[test]
    fn moving_a_directory_into_an_existing_directory_places_it_inside_by_basename() {
        let (_dir, repo) = init_repo();
        link_valid(&repo, "data/a.bin", a_hash());
        std::fs::create_dir_all(repo.join("dest")).unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let moved = mv(
            &repo_port,
            &repo,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            &cancel,
        )
        .unwrap();

        assert_eq!(moved[0].new_path, "dest/data/a.bin");
    }

    #[test]
    fn a_missing_source_reports_an_io_error_without_touching_anything() {
        let (_dir, repo) = init_repo();
        let repo_port = FsRepo::new(repo.clone());
        let cancel = Cancel::new();

        let failure = mv(
            &repo_port,
            &repo,
            Utf8Path::new("does-not-exist.bin"),
            Utf8Path::new("dst.bin"),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(*failure.error, MvError::Io { .. }));
    }
}
