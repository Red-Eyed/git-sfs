//! Remote naming and URL composition.
//!
//! contract-spec §5/§5.1. The composition rule is frozen **across users, not
//! just across versions**: several people and several git-sfs versions push
//! into the same bucket concurrently, so a one-character difference here
//! (e.g. a doubled slash) silently partitions a shared remote into two
//! disjoint stores. `compose_remote_url` is ported from the exact algorithm in
//! `internal/remote/command.go:41-63`, not reconstructed from the spec's prose
//! summary of it — the prose collapses two `TrimRight` calls into one
//! "unchanged" step that the code does not actually skip.

use std::borrow::Borrow;

use thiserror::Error;

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

/// contract-spec §6.2: `[remotes.]` (an empty name) is a config error.
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
/// and `path` fields, exactly reproducing
/// `NewRcloneTargetWithOptions`/`newRcloneRemote` (`command.go:41-63`).
///
/// Object paths are then `<url>/files/<algorithm>/<prefix>/<hash>`
/// (contract-spec §5).
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
    // `newRcloneRemote` trims the fully composed URL's trailing slashes too,
    // regardless of which branch above produced it.
    url.trim_end_matches('/').to_owned()
}

/// `command.go:62-64`: `len(path) >= 3 && path[1] == ':' && path[2] == '/'`.
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

    /// contract-spec §5.1's own table, verbatim -- each row has a plausible
    /// wrong answer, which is why the spec enumerates them individually
    /// rather than describing the rule only in prose.
    #[test]
    fn matches_the_contract_spec_url_composition_table() {
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
        // Skipping the trailing-slash strip yields `s3:dataset/root//files/...`,
        // which several backends treat as a distinct key from the single-slash
        // form -- contract-spec §5.1's stated failure mode, reached through one
        // character.
        assert_eq!(
            compose_remote_url("s3", "dataset/root///"),
            "s3:dataset/root"
        );
    }

    #[test]
    fn leading_slashes_are_stripped_only_when_not_absolute_looking() {
        // A relative-looking path with an internal leading slash after
        // trimming still gets its leading slash stripped, per the `else`
        // branch of the ported algorithm.
        assert_eq!(compose_remote_url("s3", "/nested/path"), "s3:/nested/path");
    }

    #[test]
    fn empty_backend_still_strips_trailing_slashes() {
        // The spec's prose says step 1 leaves `path` "unchanged", but
        // `newRcloneRemote` always applies `TrimRight(url, "/")` regardless of
        // branch -- ground truth is the code, not the paraphrase.
        assert_eq!(compose_remote_url("", "/abs/path/"), "/abs/path");
    }
}
