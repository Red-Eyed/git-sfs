//! Remote naming and URL composition.
//!
//! The composition rule is stable across users: several people can push into
//! the same bucket concurrently, so a one-character difference here, such as a
//! doubled slash, silently partitions a shared remote into two disjoint stores.

use std::borrow::Borrow;

use thiserror::Error;

use super::hash::{ALGORITHM, Sha256};

/// The name a remote is registered under in `config.toml`'s `[remotes.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteName(String);

/// Lets a `BTreeMap<RemoteName, _>`/`HashMap<RemoteName, _>` be looked up with
/// a plain `&str` (`map.get("default")`) instead of requiring a `RemoteName`
/// to be constructed just to query one, e.g. from a `-r`/`--remote` flag.
impl Borrow<str> for RemoteName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// The remote name every command falls back to when `-r`/`--remote` is absent.
pub const DEFAULT_REMOTE_NAME: &str = "default";

/// Empty remote names are not valid config keys.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("remote name must not be empty")]
pub struct EmptyRemoteName;

impl RemoteName {
    /// Validates a remote name.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyRemoteName`] if `name` is empty.
    pub fn parse(name: impl Into<String>) -> Result<Self, EmptyRemoteName> {
        let name = name.into();
        if name.is_empty() {
            return Err(EmptyRemoteName);
        }
        Ok(Self(name))
    }

    /// The name as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RemoteName {
    /// `"default"` — every push/pull/verify/doctor/status flag documents this
    /// as its fallback when no `-r`/`--remote` is given.
    fn default() -> Self {
        Self(DEFAULT_REMOTE_NAME.to_owned())
    }
}

impl std::fmt::Display for RemoteName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Composes the rclone target URL for a `[remotes.<name>]` entry's `backend`
/// and `path` fields.
///
/// Object paths are then `<url>/files/<algorithm>/<prefix>/<hash>`.
#[must_use]
pub fn compose_remote_url(backend: &str, path: &str) -> String {
    let url = if backend.is_empty() {
        path.to_owned()
    } else {
        let path = path.trim_end_matches('/');
        if path.starts_with('/') || is_windows_absolute(path) {
            format!("{backend}:{path}")
        } else {
            format!("{backend}:{}", path.trim_start_matches('/'))
        }
    };
    // Trim after composition as well as before, so empty backends and backend
    // URLs use the same trailing-slash rule.
    url.trim_end_matches('/').to_owned()
}

/// The remote object path for `hash`, mirroring the local cache's
/// `files/sha256/<prefix>/<hash>` layout at the far end of a
/// [`compose_remote_url`]'d `<url>`.
#[must_use]
pub fn object_url(url: &str, hash: Sha256) -> String {
    format!(
        "{url}/files/{ALGORITHM}/{}/{}",
        hash.prefix(),
        hash.to_hex()
    )
}

/// Byte indexing is safe here without a UTF-8 boundary check because the
/// bytes being compared (`:`, `/`) are single-byte ASCII regardless of what
/// surrounds them, and the length guard prevents indexing past the end.
fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_remote_name_is_rejected() {
        assert!(RemoteName::parse("").is_err());
    }

    #[test]
    fn default_remote_name_is_default() {
        assert_eq!(RemoteName::default().as_str(), "default");
    }

    /// Each row has a plausible wrong answer, so the edge cases stay explicit.
    #[test]
    fn matches_the_url_composition_table() {
        let cases = [
            ("local", "/srv/data", "local:/srv/data"),
            ("s3", "dataset/root", "s3:dataset/root"),
            ("s3", "/dataset/root/", "s3:/dataset/root"),
            ("s3", "D:/data", "s3:D:/data"),
            ("s3", "", "s3:"),
            ("", "/abs/path", "/abs/path"),
        ];
        for (backend, path, want) in cases {
            assert_eq!(
                compose_remote_url(backend, path),
                want,
                "backend={backend:?} path={path:?}"
            );
        }
    }

    #[test]
    fn trailing_slashes_never_survive_composition() {
        // Some backends treat double slashes as distinct keys, so composition
        // must remove trailing slashes before object paths are appended.
        assert_eq!(
            compose_remote_url("s3", "dataset/root///"),
            "s3:dataset/root"
        );
    }

    #[test]
    fn leading_slashes_are_stripped_only_when_not_absolute_looking() {
        // A slash-prefixed Unix path remains absolute-looking.
        assert_eq!(compose_remote_url("s3", "/nested/path"), "s3:/nested/path");
    }

    #[test]
    fn object_url_mirrors_the_local_cache_layout() {
        let hash =
            Sha256::parse("ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a")
                .unwrap();
        assert_eq!(
            object_url("s3:bucket/prefix", hash),
            format!(
                "s3:bucket/prefix/files/sha256/{}/{}",
                hash.prefix(),
                hash.to_hex()
            )
        );
    }

    #[test]
    fn empty_backend_still_strips_trailing_slashes() {
        // Empty-backend remotes still use the final trailing-slash cleanup.
        assert_eq!(compose_remote_url("", "/abs/path/"), "/abs/path");
    }
}
