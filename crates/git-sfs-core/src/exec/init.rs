//! `git-sfs init`: create committed project metadata and bind local cache.

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::Error;
use crate::domain::config::DEFAULT_TEMPLATE;
use crate::ports::{LocalStateError, bind_cache, choose_cache_root, init_cache_dirs};

const GIT_SFS_README: &str = r#"# .git-sfs

This directory is managed by git-sfs.

git-sfs stores large file bytes outside Git while Git tracks lightweight symlinks
pointing into a local cache. Do not edit the contents of this directory manually.

Committed files live here. Local machine state lives under Git's private
directory and is reached through the uncommitted `.git-sfs/cache` symlink.
"#;

/// What `init` created or confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// The config path written.
    pub config_path: Utf8PathBuf,
    /// The cache root `.git-sfs/cache` points to.
    pub cache_root: Utf8PathBuf,
}

/// Why `init` failed.
#[derive(Debug, Error)]
pub enum InitError {
    /// The config exists and `--force` was not given.
    #[error("{path} already exists; use --force to overwrite")]
    ConfigExists {
        /// Existing config path.
        path: Utf8PathBuf,
    },
    /// Local state could not be created or bound.
    #[error(transparent)]
    LocalState(#[from] LocalStateError),
    /// A file write failed.
    #[error("{path}: {source}")]
    Io {
        /// Path being written.
        path: Utf8PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl From<InitError> for Error {
    fn from(err: InitError) -> Self {
        match err {
            InitError::ConfigExists { .. } | InitError::LocalState(_) => {
                Error::Config(err.to_string())
            }
            InitError::Io { .. } => Error::Unavailable(err.to_string()),
        }
    }
}

/// Creates `.git-sfs/config.toml`, `.git-sfs/README.md`, `.gitignore` entries,
/// and a local cache binding.
///
/// # Errors
///
/// Returns [`InitError`] if existing config would be overwritten without
/// `force`, or if any filesystem operation fails.
pub fn init(
    repo: &Utf8Path,
    config_path: &Utf8Path,
    requested_cache: Option<&Utf8Path>,
    force: bool,
) -> std::result::Result<InitOutcome, InitError> {
    if config_path.exists() && !force {
        return Err(InitError::ConfigExists {
            path: config_path.to_owned(),
        });
    }

    let cache_root = choose_cache_root(repo, requested_cache)?;
    init_cache_dirs(&cache_root)?;
    bind_cache(repo, &cache_root)?;

    write_file(config_path, DEFAULT_TEMPLATE)?;
    write_file(&repo.join(".git-sfs/README.md"), GIT_SFS_README)?;
    ensure_gitignore(repo)?;

    Ok(InitOutcome {
        config_path: config_path.to_owned(),
        cache_root,
    })
}

fn ensure_gitignore(repo: &Utf8Path) -> std::result::Result<(), InitError> {
    let path = repo.join(".gitignore");
    let existing = match std::fs::read_to_string(&path) {
        Ok(existing) => existing,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(InitError::Io { path, source }),
    };
    let mut missing = Vec::new();
    for entry in [".git-sfs/cache", ".git-sfs/.cache"] {
        if !existing.lines().any(|line| line.trim() == entry) {
            missing.push(entry);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }

    let mut text = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&missing.join("\n"));
    text.push('\n');

    write_file(&path, &text)
}

fn write_file(path: &Utf8Path, text: &str) -> std::result::Result<(), InitError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| InitError::Io {
        path: parent.to_owned(),
        source,
    })?;

    use std::io::Write as _;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|source| InitError::Io {
        path: parent.to_owned(),
        source,
    })?;
    tmp.write_all(text.as_bytes())
        .map_err(|source| InitError::Io {
            path: path.to_owned(),
            source,
        })?;
    set_project_file_mode(tmp.path(), path)?;
    tmp.persist(path).map_err(|source| InitError::Io {
        path: path.to_owned(),
        source: source.error,
    })?;
    Ok(())
}

#[cfg(unix)]
fn set_project_file_mode(
    tmp_path: &std::path::Path,
    final_path: &Utf8Path,
) -> Result<(), InitError> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(tmp_path)
        .map_err(|source| InitError::Io {
            path: final_path.to_owned(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(tmp_path, permissions).map_err(|source| InitError::Io {
        path: final_path.to_owned(),
        source,
    })
}

#[cfg(not(unix))]
fn set_project_file_mode(
    _tmp_path: &std::path::Path,
    _final_path: &Utf8Path,
) -> Result<(), InitError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn creates_project_files_and_binds_the_new_default_cache() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        let config_path = repo.join(".git-sfs/config.toml");

        let outcome = init(&repo, &config_path, None, false).unwrap();

        assert_eq!(outcome.cache_root, repo.join(".git/sfs/cache"));
        assert!(repo.join(".git-sfs/config.toml").is_file());
        assert!(repo.join(".git-sfs/README.md").is_file());
        assert!(repo.join(".git/sfs/cache/files/sha256").is_dir());
        assert_eq!(
            std::fs::read_link(repo.join(".git-sfs/cache")).unwrap(),
            std::fs::canonicalize(repo.join(".git/sfs/cache")).unwrap()
        );
        let gitignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".git-sfs/cache"));
        assert!(gitignore.contains(".git-sfs/.cache"));
    }

    #[cfg(unix)]
    #[test]
    fn project_files_are_world_readable_like_normal_tracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        init(&repo, &repo.join(".git-sfs/config.toml"), None, false).unwrap();

        for path in [
            repo.join(".git-sfs/config.toml"),
            repo.join(".git-sfs/README.md"),
            repo.join(".gitignore"),
        ] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }
    }

    #[test]
    fn refuses_to_overwrite_config_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        let config_path = repo.join(".git-sfs/config.toml");

        init(&repo, &config_path, None, false).unwrap();

        assert!(matches!(
            init(&repo, &config_path, None, false),
            Err(InitError::ConfigExists { .. })
        ));
        assert!(init(&repo, &config_path, None, true).is_ok());
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

        let outcome = init(&repo, &repo.join(".git-sfs/config.toml"), None, false).unwrap();

        assert_eq!(
            outcome.cache_root,
            Utf8PathBuf::from_path_buf(std::fs::canonicalize(cache).unwrap()).unwrap()
        );
    }
}
