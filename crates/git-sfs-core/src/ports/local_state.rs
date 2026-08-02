//! Reading already-existing local, per-machine state — contract-spec §7.
//! Repository discovery and machine-local state.
//!
//! Normal commands intentionally resolve one cache source: the repo-facing
//! `.git-sfs/cache` symlink. `init` and `setup` are the only commands that
//! choose or bind that symlink.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::domain::hash::ALGORITHM;
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
    #[error("no cache configured: run git-sfs setup")]
    MissingCacheConfig,
    /// The `.git-sfs/cache` symlink exists but its target is not valid
    /// UTF-8 — unrepresentable as the `Utf8PathBuf` this crate's paths are
    /// throughout.
    #[error("cache link target at {link} is not valid UTF-8")]
    NonUtf8CacheTarget {
        /// The symlink whose target could not be read as UTF-8.
        link: Utf8PathBuf,
    },
    /// A `.git` file exists, but it is not the `gitdir: ...` pointer shape
    /// Git writes for worktrees and submodules.
    #[error("{path}: unsupported .git file format")]
    InvalidGitDirFile {
        /// The `.git` file that could not be interpreted.
        path: Utf8PathBuf,
    },
    /// The repo already has a cache binding and the caller tried to point it
    /// somewhere else.
    #[error("cache link {link} points to {existing}, not {target}")]
    CacheRebind {
        /// The existing `.git-sfs/cache` link.
        link: Utf8PathBuf,
        /// Its existing canonical target.
        existing: Utf8PathBuf,
        /// The requested canonical target.
        target: Utf8PathBuf,
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
            | LocalStateError::NonUtf8CacheTarget { .. }
            | LocalStateError::InvalidGitDirFile { .. }
            | LocalStateError::CacheRebind { .. } => Error::Config(err.to_string()),
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

/// Resolves the already-bound cache root from `.git-sfs/cache`.
///
/// # Errors
///
/// Returns [`LocalStateError::MissingCacheConfig`] if the symlink is absent,
/// [`LocalStateError::NonUtf8CacheTarget`] if the symlink's target cannot be
/// read as UTF-8, and [`LocalStateError::Io`] if the symlink exists but could
/// not be read for any other reason.
pub fn resolve_cache_root(repo: &Utf8Path) -> Result<Utf8PathBuf, LocalStateError> {
    let link = cache_link(repo);
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

/// Creates `.git-sfs/`, the committed project-metadata directory.
///
/// # Errors
///
/// Returns [`LocalStateError::Io`] if the directory cannot be created.
pub fn init_git_sfs_dir(repo: &Utf8Path) -> Result<(), LocalStateError> {
    let dir = repo.join(".git-sfs");
    std::fs::create_dir_all(&dir).map_err(|source| LocalStateError::Io { path: dir, source })
}

/// Creates the standard cache subdirectories under `cache_root`.
///
/// # Errors
///
/// Returns [`LocalStateError::Io`] if any directory cannot be created.
pub fn init_cache_dirs(cache_root: &Utf8Path) -> Result<(), LocalStateError> {
    for path in [
        cache_root.join("files").join(ALGORITHM),
        cache_root.join("tmp"),
        cache_root.join("locks"),
    ] {
        std::fs::create_dir_all(&path).map_err(|source| LocalStateError::Io { path, source })?;
    }
    Ok(())
}

/// Chooses the cache root `init`/`setup` should bind.
///
/// Explicit `--cache` wins. Without it, existing repos stay where they are:
/// an existing `.git-sfs/cache` binding is preserved, then the old v1 default
/// `.git-sfs/.cache` is recognized, and only then does v2 choose its new
/// private-Git-dir default.
///
/// # Errors
///
/// Returns [`LocalStateError`] if the existing cache link or Git private
/// directory cannot be read.
pub fn choose_cache_root(
    repo: &Utf8Path,
    requested: Option<&Utf8Path>,
) -> Result<Utf8PathBuf, LocalStateError> {
    if let Some(requested) = requested {
        return Ok(absolute(requested));
    }
    match resolve_cache_root(repo) {
        Ok(cache_root) => return Ok(cache_root),
        Err(LocalStateError::MissingCacheConfig) => {}
        Err(err) => return Err(err),
    }
    let old = old_default_cache_root(repo);
    if old.exists() {
        return Ok(old);
    }
    default_cache_root(repo)
}

/// Binds `.git-sfs/cache` to `cache_root`, rejecting rebinding.
///
/// # Errors
///
/// Returns [`LocalStateError::CacheRebind`] if the link already points
/// elsewhere, or [`LocalStateError::Io`] if the symlink cannot be read or
/// created.
pub fn bind_cache(repo: &Utf8Path, cache_root: &Utf8Path) -> Result<(), LocalStateError> {
    init_git_sfs_dir(repo)?;
    let link = cache_link(repo);
    let target = canonical_path(cache_root);

    match std::fs::read_link(&link) {
        Ok(existing) => {
            let existing = Utf8PathBuf::from_path_buf(existing)
                .map_err(|_| LocalStateError::NonUtf8CacheTarget { link: link.clone() })?;
            let existing = if existing.is_absolute() {
                existing
            } else {
                let link_dir = link.parent().unwrap_or(repo);
                clean_utf8(&link_dir.join(existing))
            };
            let existing = canonical_path(&existing);
            if existing == target {
                return Ok(());
            }
            return Err(LocalStateError::CacheRebind {
                link,
                existing,
                target,
            });
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(LocalStateError::Io { path: link, source }),
    }

    std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path())
        .map_err(|source| LocalStateError::Io { path: link, source })
}

fn cache_link(repo: &Utf8Path) -> Utf8PathBuf {
    repo.join(".git-sfs").join("cache")
}

fn old_default_cache_root(repo: &Utf8Path) -> Utf8PathBuf {
    repo.join(".git-sfs").join(".cache")
}

fn default_cache_root(repo: &Utf8Path) -> Result<Utf8PathBuf, LocalStateError> {
    Ok(private_git_dir(repo)?.join("sfs").join("cache"))
}

fn private_git_dir(repo: &Utf8Path) -> Result<Utf8PathBuf, LocalStateError> {
    let git_entry = repo.join(".git");
    let metadata = std::fs::symlink_metadata(&git_entry).map_err(|source| LocalStateError::Io {
        path: git_entry.clone(),
        source,
    })?;
    if metadata.is_dir() {
        return Ok(git_entry);
    }

    let text = std::fs::read_to_string(&git_entry).map_err(|source| LocalStateError::Io {
        path: git_entry.clone(),
        source,
    })?;
    let Some(rest) = text.strip_prefix("gitdir:") else {
        return Err(LocalStateError::InvalidGitDirFile { path: git_entry });
    };
    let path = Utf8Path::new(rest.trim());
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(clean_utf8(&repo.join(path)))
    }
}

fn canonical_path(path: &Utf8Path) -> Utf8PathBuf {
    std::fs::canonicalize(path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| clean_utf8(path))
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

        let resolved = resolve_cache_root(&repo).unwrap();
        assert_eq!(resolved, cache_target);
    }

    #[test]
    fn resolve_cache_root_resolves_a_relative_symlink_target_against_the_links_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();
        std::os::unix::fs::symlink("../cache-dir", repo.join(".git-sfs/cache").as_std_path())
            .unwrap();

        let resolved = resolve_cache_root(&repo).unwrap();
        assert_eq!(resolved, repo.join("cache-dir"));
    }

    #[test]
    fn resolve_cache_root_fails_when_nothing_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();

        assert!(matches!(
            resolve_cache_root(&repo),
            Err(LocalStateError::MissingCacheConfig)
        ));
    }

    #[test]
    fn choose_cache_root_uses_a_requested_cache() {
        let repo = Utf8PathBuf::from("/repo");
        let requested = Utf8PathBuf::from("/cache");

        assert_eq!(
            choose_cache_root(&repo, Some(&requested)).unwrap(),
            requested
        );
    }

    #[test]
    fn choose_cache_root_preserves_an_existing_binding() {
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

        assert_eq!(choose_cache_root(&repo, None).unwrap(), cache_target);
    }

    #[test]
    fn choose_cache_root_recognizes_the_old_v1_default_when_no_link_exists() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs/.cache")).unwrap();

        assert_eq!(
            choose_cache_root(&repo, None).unwrap(),
            repo.join(".git-sfs/.cache")
        );
    }

    #[test]
    fn choose_cache_root_defaults_under_the_private_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        assert_eq!(
            choose_cache_root(&repo, None).unwrap(),
            repo.join(".git/sfs/cache")
        );
    }

    #[test]
    fn choose_cache_root_uses_the_gitdir_pointer_for_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("worktree")).unwrap();
        let git_dir = Utf8PathBuf::from_path_buf(dir.path().join("real-git")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: ../real-git\n").unwrap();

        assert_eq!(
            choose_cache_root(&repo, None).unwrap(),
            git_dir.join("sfs/cache")
        );
    }

    #[test]
    fn init_cache_dirs_creates_the_cache_layout() {
        let dir = tempfile::tempdir().unwrap();
        let cache_root = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();

        init_cache_dirs(&cache_root).unwrap();

        assert!(cache_root.join("files/sha256").is_dir());
        assert!(cache_root.join("tmp").is_dir());
        assert!(cache_root.join("locks").is_dir());
    }

    #[test]
    fn bind_cache_writes_a_canonical_absolute_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&cache).unwrap();

        bind_cache(&repo, &cache).unwrap();

        let target = std::fs::read_link(repo.join(".git-sfs/cache")).unwrap();
        assert_eq!(target, std::fs::canonicalize(cache).unwrap());
    }

    #[test]
    fn bind_cache_rebinding_to_the_same_target_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&cache).unwrap();

        bind_cache(&repo, &cache).unwrap();
        bind_cache(&repo, &cache).unwrap();
    }

    #[test]
    fn bind_cache_rebinding_to_an_equivalent_target_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let equivalent = cache.join("../cache");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&cache).unwrap();

        bind_cache(&repo, &cache).unwrap();
        bind_cache(&repo, &equivalent).unwrap();
    }

    #[test]
    fn bind_cache_refuses_to_repoint_an_existing_link() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let first = Utf8PathBuf::from_path_buf(dir.path().join("first")).unwrap();
        let second = Utf8PathBuf::from_path_buf(dir.path().join("second")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&first).unwrap();
        init_cache_dirs(&second).unwrap();

        bind_cache(&repo, &first).unwrap();

        assert!(matches!(
            bind_cache(&repo, &second),
            Err(LocalStateError::CacheRebind { .. })
        ));
    }
}
