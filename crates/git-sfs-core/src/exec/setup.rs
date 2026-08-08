//! `git-sfs setup`: bind clone-local cache state.
//!
//! Setup is intentionally structural. Tracked file symlinks already point
//! through `.git-sfs/cache`, so this command does not scan, hash, probe, or
//! rewrite them. Its job is to make that repo-facing cache symlink exist.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::ports::{
    LocalStateError, Lock, LockError, LockName, bind_cache, choose_cache_root, init_cache_dirs,
    init_git_sfs_dir,
};
use crate::{Cancel, Error};

/// What `setup` created or confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    /// The cache root `.git-sfs/cache` points to.
    pub cache_root: Utf8PathBuf,
}

/// Why `setup` failed.
#[derive(Debug, Error)]
pub enum SetupError {
    /// Local state could not be created or bound.
    #[error(transparent)]
    LocalState(#[from] LocalStateError),
    /// The setup lock could not be acquired.
    #[error(transparent)]
    Lock(#[from] LockError),
}

impl From<SetupError> for Error {
    fn from(err: SetupError) -> Self {
        match err {
            SetupError::LocalState(_) => Error::Config(err.to_string()),
            SetupError::Lock(err) => err.into(),
        }
    }
}

/// Ensures `.git-sfs/cache` points at a local cache root and initializes that
/// cache's standard subdirectories.
///
/// # Errors
///
/// Returns [`SetupError`] if the cache binding cannot be chosen/bound or the
/// setup lock cannot be acquired.
pub fn setup(
    repo: &Utf8Path,
    requested_cache: Option<&Utf8Path>,
    cancel: &Cancel,
) -> std::result::Result<SetupOutcome, SetupError> {
    init_git_sfs_dir(repo)?;
    let cache_root = choose_cache_root(repo, requested_cache)?;
    init_cache_dirs(&cache_root)?;

    let _lock = Lock::acquire(&cache_root.join("locks"), LockName::Setup, cancel)?;
    bind_cache(repo, &cache_root)?;

    Ok(SetupOutcome { cache_root })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_the_new_default_cache_when_no_local_state_exists() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        let outcome = setup(&repo, None, &Cancel::new()).unwrap();

        assert_eq!(outcome.cache_root, repo.join(".git/sfs/cache"));
        assert!(repo.join(".git/sfs/cache/files/sha256").is_dir());
        assert_eq!(
            std::fs::read_link(repo.join(".git-sfs/cache")).unwrap(),
            std::fs::canonicalize(repo.join(".git/sfs/cache")).unwrap()
        );
    }

    #[test]
    fn preserves_an_existing_cache_binding() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("external-cache")).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&cache).unwrap();
        bind_cache(&repo, &cache).unwrap();

        let outcome = setup(&repo, None, &Cancel::new()).unwrap();

        assert_eq!(
            outcome.cache_root,
            Utf8PathBuf::from_path_buf(std::fs::canonicalize(cache).unwrap()).unwrap()
        );
    }

    #[test]
    fn recognizes_old_cache_dir_when_link_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs/.cache")).unwrap();

        let outcome = setup(&repo, None, &Cancel::new()).unwrap();

        assert_eq!(outcome.cache_root, repo.join(".git-sfs/.cache"));
        assert_eq!(
            std::fs::read_link(repo.join(".git-sfs/cache")).unwrap(),
            std::fs::canonicalize(repo.join(".git-sfs/.cache")).unwrap()
        );
    }

    #[test]
    fn refuses_to_rebind_an_existing_cache() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("repo")).unwrap();
        let first = Utf8PathBuf::from_path_buf(dir.path().join("first")).unwrap();
        let second = Utf8PathBuf::from_path_buf(dir.path().join("second")).unwrap();
        std::fs::create_dir_all(repo.join(".git-sfs")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        init_cache_dirs(&first).unwrap();
        init_cache_dirs(&second).unwrap();
        bind_cache(&repo, &first).unwrap();

        assert!(matches!(
            setup(&repo, Some(&second), &Cancel::new()),
            Err(SetupError::LocalState(LocalStateError::CacheRebind { .. }))
        ));
    }
}
