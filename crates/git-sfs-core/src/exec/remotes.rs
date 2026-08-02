//! `git-sfs remotes`: list configured remotes without contacting them.
//!
//! This command reads only `.git-sfs/config.toml`, the committed source of
//! truth. Connectivity and rclone preflight checks belong to `doctor` and the
//! byte-moving commands, not here.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use thiserror::Error;

use crate::domain::config::{self, Config, ConfigError};
use crate::domain::remote::DEFAULT_REMOTE_NAME;
use crate::error::Error;

/// One configured remote, flattened for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteEntry {
    /// The `[remotes.<name>]` key.
    pub name: String,
    /// The rclone backend name from the remote config.
    pub backend: String,
    /// The optional path within the backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The optional rclone config path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Whether this is the implicit remote used when `-r`/`--remote` is absent.
    pub default: bool,
}

/// Why listing remotes failed.
#[derive(Debug, Error)]
pub enum RemotesError {
    /// The config file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The config path.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The config file was present but invalid.
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl From<RemotesError> for Error {
    fn from(err: RemotesError) -> Self {
        match err {
            RemotesError::Io { path, source } if source.kind() == io::ErrorKind::NotFound => {
                Error::Config(format!("config file not found: {path} (run git-sfs init)"))
            }
            RemotesError::Io { path, source } => {
                Error::Unavailable(format!("open config {path}: {source}"))
            }
            RemotesError::Config(err) => Error::Config(err.to_string()),
        }
    }
}

/// Reads `config_path` and returns its configured remotes, sorted by name.
///
/// # Errors
///
/// Returns [`RemotesError::Io`] if the config cannot be read and
/// [`RemotesError::Config`] if its contents do not validate.
pub fn remotes(config_path: &Utf8Path) -> Result<Vec<RemoteEntry>, RemotesError> {
    let text = std::fs::read_to_string(config_path).map_err(|source| RemotesError::Io {
        path: config_path.to_owned(),
        source,
    })?;
    let config = config::parse_and_validate(&text)?;
    Ok(remote_entries(&config))
}

fn remote_entries(config: &Config) -> Vec<RemoteEntry> {
    config
        .remotes
        .iter()
        .map(|(name, remote)| RemoteEntry {
            name: name.as_str().to_owned(),
            backend: remote.backend.clone(),
            path: remote.path.clone(),
            config: remote
                .rclone_config_path
                .as_ref()
                .map(std::string::ToString::to_string),
            default: name.as_str() == DEFAULT_REMOTE_NAME,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_REMOTES: &str = r#"version = 1

[remotes.zed]
backend = "archive"
path = "old"

[remotes.default]
backend = "myremote"
path = "datasets/project"
config = "rclone.conf"

[settings]
algorithm = "sha256"
"#;

    #[test]
    fn entries_are_sorted_and_flattened_for_display() {
        let config = config::parse_and_validate(TWO_REMOTES).unwrap();
        let entries = remote_entries(&config);

        assert_eq!(
            entries,
            vec![
                RemoteEntry {
                    name: "default".to_owned(),
                    backend: "myremote".to_owned(),
                    path: Some("datasets/project".to_owned()),
                    config: Some("rclone.conf".to_owned()),
                    default: true,
                },
                RemoteEntry {
                    name: "zed".to_owned(),
                    backend: "archive".to_owned(),
                    path: Some("old".to_owned()),
                    config: None,
                    default: false,
                },
            ]
        );
    }

    #[test]
    fn reads_config_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("config.toml")).unwrap();
        std::fs::write(&path, TWO_REMOTES).unwrap();

        let entries = remotes(&path).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "default");
        assert_eq!(entries[1].name, "zed");
    }

    #[test]
    fn lists_remote_configuration_without_contacting_the_backend() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("config.toml")).unwrap();
        std::fs::write(
            &path,
            r#"version = 1

[remotes.default]
backend = "backend-that-should-not-exist"
path = "datasets/project"
config = "missing-rclone.conf"

[settings]
algorithm = "sha256"
"#,
        )
        .unwrap();

        let entries = remotes(&path).unwrap();

        assert_eq!(
            entries,
            vec![RemoteEntry {
                name: "default".to_owned(),
                backend: "backend-that-should-not-exist".to_owned(),
                path: Some("datasets/project".to_owned()),
                config: Some("missing-rclone.conf".to_owned()),
                default: true,
            }]
        );
    }
}
