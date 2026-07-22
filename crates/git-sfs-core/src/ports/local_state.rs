//! Reading already-existing local, per-machine state — contract-spec §7.
//! Both functions here are frozen mechanism with no open design question:
//! repository discovery walks upward for a `.git` entry (§7.1), and cache
//! resolution follows a strict three-source precedence (§7.2).
//!
//! Deliberately **not** cache *creation* or *binding* — that remains an open
//! design question (the `init`/`setup` commands' own purpose is still being
//! reconsidered). Keeping this module to read-only resolution lets any
//! command that only needs an already-bound cache (`add`, and eventually
//! `push`/`pull`/`status`/`verify`) proceed without waiting on that decision;
//! if nothing is bound yet, resolution simply fails with
//! [`LocalStateError::MissingCacheConfig`], matching v1's exact behavior.
//!
//! Side-effecting inputs (the current directory, the `GIT_SFS_CACHE`
//! environment variable) are read once by the caller and passed in, rather
//! than read internally here — the same "inject the side-effecting
//! dependency" reasoning [`super::remote`]'s `RcloneRemote` tests lean on:
//! reading `std::env::var` from inside a library function is invisible
//! global state a caller cannot substitute for a test, and mutating a real
//! process-global env var from a test is unsafe under `cargo test`'s
//! parallel execution.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::domain::symlink::clean_utf8;
use crate::error::Error;

/// Why local-state resolution failed.
#[derive(Debug, Error)]
pub enum LocalStateError {
    /// No `.git` entry was found in `start` or any ancestor directory
    /// (contract-spec §7.1).
    #[error("not a git repository: no .git found in {start} or any parent directory")]
    NoRepository {
        /// Where the upward search started.
        start: Utf8PathBuf,
    },
    /// None of the three precedence sources resolved a cache
    /// (contract-spec §7.2).
    #[error("no cache configured: pass --cache, set GIT_SFS_CACHE, or run git-sfs setup")]
    MissingCacheConfig,
    /// The `.git-sfs/cache` symlink exists but its target is not valid
    /// UTF-8 — unrepresentable as the `Utf8PathBuf` this crate's paths are
    /// throughout.
    #[error("cache link target at {link} is not valid UTF-8")]
    NonUtf8CacheTarget {
        /// The symlink whose target could not be read as UTF-8.
        link: Utf8PathBuf,
    },
    /// An I/O operation failed while resolving local state.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl From<LocalStateError> for Error {
    fn from(err: LocalStateError) -> Self {
        match err {
            LocalStateError::NoRepository { .. }
            | LocalStateError::MissingCacheConfig
            | LocalStateError::NonUtf8CacheTarget { .. } => Error::Config(err.to_string()),
            LocalStateError::Io { .. } => Error::Unavailable(err.to_string()),
        }
    }
}

/// Walks upward from `start` until a `.git` entry is found, returning that
/// directory as the repository root.
///
/// `.git` may be a directory (a normal repository) or a file (a submodule or
/// worktree pointer) — both are accepted, matching v1's `os.Stat` rather
/// than a directory-only check (contract-spec §7.1).
///
/// # Errors
///
/// Returns [`LocalStateError::NoRepository`] if the filesystem root is
/// reached with no `.git` found, and [`LocalStateError::Io`] if a stat along
/// the way fails for a reason other than "not found".
pub fn discover_repo(start: &Utf8Path) -> Result<Utf8PathBuf, LocalStateError> {
    let mut dir = start.to_owned();
    loop {
        let git_entry = dir.join(".git");
        match std::fs::symlink_metadata(&git_entry) {
            Ok(_) => return Ok(dir),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(LocalStateError::Io {
                    path: git_entry,
                    source,
                });
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_owned(),
            None => {
                return Err(LocalStateError::NoRepository {
                    start: start.to_owned(),
                });
            }
        }
    }
}

/// Resolves the cache root by contract-spec §7.2's strict precedence:
///
/// 1. `cache_flag`, if given (made absolute if it is not already)
/// 2. `git_sfs_cache_env`, if given and non-empty (made absolute)
/// 3. The `.git-sfs/cache` symlink's target, if the symlink exists — already
///    absolute per contract-spec §2's write-side contract, but a relative
///    target is still resolved against the link's own directory for
///    robustness, matching v1
///
/// A missing symlink at step 3 is not itself an error — it falls through to
/// [`LocalStateError::MissingCacheConfig`], matching v1's "empty value"
/// case (§7.2).
///
/// # Errors
///
/// Returns [`LocalStateError::MissingCacheConfig`] if none of the three
/// sources resolve, [`LocalStateError::NonUtf8CacheTarget`] if the symlink's
/// target cannot be read as UTF-8, and [`LocalStateError::Io`] if the
/// symlink exists but could not be read for any other reason.
pub fn resolve_cache_root(
    repo: &Utf8Path,
    cache_flag: Option<&Utf8Path>,
    git_sfs_cache_env: Option<&str>,
) -> Result<Utf8PathBuf, LocalStateError> {
    if let Some(flag) = cache_flag {
        return Ok(absolute(flag));
    }
    if let Some(env_value) = git_sfs_cache_env
        && !env_value.is_empty()
    {
        return Ok(absolute(Utf8Path::new(env_value)));
    }

    let link = repo.join(".git-sfs").join("cache");
    let target = match std::fs::read_link(&link) {
        Ok(target) => target,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(LocalStateError::MissingCacheConfig);
        }
        Err(source) => return Err(LocalStateError::Io { path: link, source }),
    };
    let target = Utf8PathBuf::from_path_buf(target)
        .map_err(|_| LocalStateError::NonUtf8CacheTarget { link: link.clone() })?;

    if target.is_absolute() {
        Ok(target)
    } else {
        let link_dir = link.parent().unwrap_or(repo);
        Ok(clean_utf8(&link_dir.join(target)))
    }
}

/// `filepath.Abs`-equivalent: joins a relative path against the current
/// directory, without resolving symlinks. Falls back to the original path
/// unchanged if the current directory cannot be determined, matching v1's
/// `abs()` (`localstate.go:89-95`).
fn absolute(path: &Utf8Path) -> Utf8PathBuf {
    match std::path::absolute(path.as_std_path()) {
        Ok(abs) => Utf8PathBuf::from_path_buf(abs).unwrap_or_else(|_| path.to_owned()),
        Err(_) => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_repo_finds_a_git_directory_in_the_starting_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let start = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();

        assert_eq!(discover_repo(&start).unwrap(), start);
    }

    #[test]
    fn discover_repo_walks_upward_through_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let start = Utf8PathBuf::from_path_buf(nested).unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();

        assert_eq!(discover_repo(&start).unwrap(), repo);
    }

    #[test]
    fn discover_repo_accepts_a_git_file_not_just_a_directory() {
        // Submodules and worktrees have a `.git` *file* containing a
        // `gitdir:` pointer, not a directory -- contract-spec §7.1 requires
        // both to be accepted.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
        let start = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();

        assert_eq!(discover_repo(&start).unwrap(), start);
    }

    #[test]
    fn discover_repo_fails_when_no_git_exists_up_to_the_root() {
        // /tmp itself (or any tempdir under it) is never a git repository,
        // so walking all the way to `/` must fail rather than loop forever.
        let dir = tempfile::tempdir().unwrap();
        let start = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();

        assert!(matches!(
            discover_repo(&start),
            Err(LocalStateError::NoRepository { .. })
        ));
    }

    #[test]
    fn resolve_cache_root_prefers_the_flag_over_everything_else() {
        let repo = Utf8PathBuf::from("/repo");
        let flag = Utf8PathBuf::from("/from-flag");
        let resolved = resolve_cache_root(&repo, Some(&flag), Some("/from-env")).unwrap();
        assert_eq!(resolved, flag);
    }

    #[test]
    fn resolve_cache_root_falls_back_to_the_env_var() {
        let repo = Utf8PathBuf::from("/repo");
        let resolved = resolve_cache_root(&repo, None, Some("/from-env")).unwrap();
        assert_eq!(resolved, Utf8PathBuf::from("/from-env"));
    }

    #[test]
    fn resolve_cache_root_falls_back_to_the_bound_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let cache_target = Utf8PathBuf::from_path_buf(cache_dir.path().to_owned()).unwrap();
        std::os::unix::fs::symlink(
            cache_target.as_std_path(),
            repo.join(".git-sfs/cache").as_std_path(),
        )
        .unwrap();

        let resolved = resolve_cache_root(&repo, None, None).unwrap();
        assert_eq!(resolved, cache_target);
    }

    #[test]
    fn resolve_cache_root_resolves_a_relative_symlink_target_against_the_links_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();
        std::os::unix::fs::symlink("../cache-dir", repo.join(".git-sfs/cache").as_std_path())
            .unwrap();

        let resolved = resolve_cache_root(&repo, None, None).unwrap();
        assert_eq!(resolved, repo.join("cache-dir"));
    }

    #[test]
    fn resolve_cache_root_fails_when_nothing_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();

        assert!(matches!(
            resolve_cache_root(&repo, None, None),
            Err(LocalStateError::MissingCacheConfig)
        ));
    }

    #[test]
    fn resolve_cache_root_ignores_an_empty_env_var() {
        // Matches v1: an unset *or empty* GIT_SFS_CACHE both fall through,
        // rather than resolving to "".
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();

        assert!(matches!(
            resolve_cache_root(&repo, None, Some("")),
            Err(LocalStateError::MissingCacheConfig)
        ));
    }
}
