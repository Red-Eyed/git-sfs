//! A faithful port of v1's hand-rolled config line scanner
//! (`internal/config/config.go:153-306`).
//!
//! This is the deliberate exception to "use a crate, don't reinvent the
//! parser": no generic TOML implementation replicates v1's specific
//! quirks — comment-stripping that doesn't know about quotes, quote-trimming
//! that doesn't know about escapes — and reproducing those quirks exactly is
//! the entire point of this module. contract-spec §6.3 requires it: v2 must
//! keep reading configs the way v1 did, at least well enough to detect where
//! the real `toml` crate would read one differently.
//!
//! Two things per string field are captured, not one:
//! - `v1_reading`: what v1's pipeline actually computes and uses (goes through
//!   comment-stripping and quote-trimming, exactly as Go does it).
//! - `as_written`: the trimmed value text from the *original*, un-stripped
//!   line — needed only to render contract-spec §6.5's ambiguity message,
//!   which shows the value as the user actually typed it, not v1's mangled
//!   reading of it.

use std::collections::BTreeMap;

use thiserror::Error;

/// A string-valued field, with both what v1 computed and what was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StringField {
    pub v1_reading: String,
    pub as_written: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LegacyRemote {
    pub backend: Option<StringField>,
    pub path: Option<StringField>,
    pub config: Option<StringField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LegacySettings {
    pub algorithm: Option<StringField>,
    pub n_jobs: Option<i64>,
    pub retry_max: Option<i64>,
    pub min_rclone_version: Option<StringField>,
    pub min_git_sfs_version: Option<StringField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LegacyConfig {
    pub remotes: BTreeMap<String, LegacyRemote>,
    pub settings: LegacySettings,
}

/// Mirrors every error `Load` can produce (`config.go:153-271`), collapsed to
/// one enum since — unlike the top-level [`super::ConfigError`] — nothing
/// downstream branches on which case this is; it only needs to know *that*
/// the legacy scanner failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum LegacyScanError {
    #[error("invalid config line {line:?}")]
    InvalidLine { line: String },
    #[error("invalid remote section {name:?}")]
    EmptyRemoteName { name: String },
    #[error(".git-sfs/config.toml must not contain local cache configuration")]
    CacheNotAllowed,
    #[error("unknown .git-sfs/config.toml section {name:?}")]
    UnknownSection { name: String },
    #[error("unknown .git-sfs/config.toml field {key:?}")]
    UnknownTopLevelField { key: String },
    #[error("unknown settings field {key:?}")]
    UnknownSettingsField { key: String },
    #[error("unknown remote field {key:?}")]
    UnknownRemoteField { key: String },
    #[error("remote field {key:?} appears before remote name")]
    FieldBeforeRemoteName { key: String },
    #[error("unsupported .git-sfs/config.toml version {value:?}")]
    UnsupportedVersion { value: String },
    #[error(".git-sfs/config.toml must declare version = 1")]
    MissingVersion,
    #[error("invalid settings n_jobs {value:?}")]
    NJobsNotAnInteger { value: String },
    #[error("settings n_jobs must be >= 0")]
    NJobsNegative,
    #[error("invalid settings retry_max {value:?}")]
    RetryMaxNotAnInteger { value: String },
    #[error("unsupported hash algorithm {value:?}")]
    UnsupportedAlgorithm { value: String },
    #[error("remote {name:?} requires backend")]
    RemoteMissingBackend { name: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    TopLevel,
    Settings,
    Remotes,
}

/// Runs v1's exact scan-and-validate pipeline over `text`, producing the same
/// result `Load` would (minus the file I/O, which the caller already did).
pub(super) fn scan(text: &str) -> Result<LegacyConfig, LegacyScanError> {
    let mut cfg = LegacyConfig::default();
    let mut section = Section::TopLevel;
    let mut current_remote: Option<String> = None;
    let mut version_field: Option<String> = None;

    for raw_line in text.lines() {
        let stripped = strip_comment(raw_line);
        if stripped.trim().is_empty() {
            continue;
        }
        let line = stripped.trim();

        if let Some(name) = section_header(line) {
            if name == "settings" {
                section = Section::Settings;
                current_remote = None;
            } else if let Some(remote) = name.strip_prefix("remotes.") {
                if remote.is_empty() {
                    return Err(LegacyScanError::EmptyRemoteName {
                        name: name.to_owned(),
                    });
                }
                section = Section::Remotes;
                current_remote = Some(remote.to_owned());
                cfg.remotes.entry(remote.to_owned()).or_default();
            } else if name == "cache" || name.starts_with("cache.") {
                return Err(LegacyScanError::CacheNotAllowed);
            } else {
                return Err(LegacyScanError::UnknownSection {
                    name: name.to_owned(),
                });
            }
            continue;
        }

        let Some((key, v1_value)) = split_field(line) else {
            return Err(LegacyScanError::InvalidLine {
                line: line.to_owned(),
            });
        };
        // `as_written` is the value text from the *original*, non-comment-
        // stripped line, quotes intact -- unlike `v1_value`, it does not go
        // through `unquote`, since the ambiguity message needs to show the
        // value exactly as the user typed it, not v1's mangled reading of it.
        let as_written = raw_value_text(raw_line.trim()).unwrap_or_else(|| v1_value.clone());
        let field = |v1_reading: String| StringField {
            v1_reading,
            as_written,
        };

        match section {
            Section::TopLevel if key == "version" => version_field = Some(v1_value),
            Section::TopLevel if key == "cache" => return Err(LegacyScanError::CacheNotAllowed),
            Section::TopLevel => {
                return Err(LegacyScanError::UnknownTopLevelField {
                    key: key.to_owned(),
                });
            }

            Section::Settings => match key {
                "algorithm" => cfg.settings.algorithm = Some(field(v1_value)),
                "n_jobs" => {
                    cfg.settings.n_jobs =
                        Some(
                            v1_value
                                .parse()
                                .map_err(|_| LegacyScanError::NJobsNotAnInteger {
                                    value: v1_value.clone(),
                                })?,
                        );
                }
                "retry_max" => {
                    cfg.settings.retry_max = Some(v1_value.parse().map_err(|_| {
                        LegacyScanError::RetryMaxNotAnInteger {
                            value: v1_value.clone(),
                        }
                    })?);
                }
                "min_rclone_version" => cfg.settings.min_rclone_version = Some(field(v1_value)),
                "min_git_sfs_version" => cfg.settings.min_git_sfs_version = Some(field(v1_value)),
                _ => {
                    return Err(LegacyScanError::UnknownSettingsField {
                        key: key.to_owned(),
                    });
                }
            },

            Section::Remotes => {
                // Structurally unreachable, same as `config.go:233-235`: entering
                // `Remotes` and setting `current_remote` happen together at the
                // section header, so no key/value line can see one without the
                // other. Kept anyway, matching the Go source, as a guard against
                // a future edit to the state machine silently making it reachable.
                let Some(remote) = &current_remote else {
                    return Err(LegacyScanError::FieldBeforeRemoteName {
                        key: key.to_owned(),
                    });
                };
                let entry = cfg
                    .remotes
                    .get_mut(remote)
                    .expect("remote entry created at its section header");
                match key {
                    "backend" => entry.backend = Some(field(v1_value)),
                    "path" => entry.path = Some(field(v1_value)),
                    "config" => entry.config = Some(field(v1_value)),
                    _ => {
                        return Err(LegacyScanError::UnknownRemoteField {
                            key: key.to_owned(),
                        });
                    }
                }
            }
        }
    }

    match version_field.as_deref() {
        Some("1") => {}
        Some(other) => {
            return Err(LegacyScanError::UnsupportedVersion {
                value: other.to_owned(),
            });
        }
        None => return Err(LegacyScanError::MissingVersion),
    }

    if cfg.settings.n_jobs.is_some_and(|n| n < 0) {
        return Err(LegacyScanError::NJobsNegative);
    }
    let algorithm = cfg
        .settings
        .algorithm
        .as_ref()
        .map(|f| f.v1_reading.as_str());
    if let Some(algorithm) = algorithm
        && algorithm != "sha256"
    {
        return Err(LegacyScanError::UnsupportedAlgorithm {
            value: algorithm.to_owned(),
        });
    }
    for (name, remote) in &cfg.remotes {
        if remote.backend.is_none() {
            return Err(LegacyScanError::RemoteMissingBackend { name: name.clone() });
        }
    }

    Ok(cfg)
}

/// `config.go:301-306`: the first `#` anywhere in the line ends it — this
/// runs before any quote-awareness exists, which is the entire mechanism
/// behind contract-spec §6.3's divergence.
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// `config.go:172-173`: a line that starts with `[` and ends with `]` (after
/// trimming) is a section header naming the text between them.
fn section_header(line: &str) -> Option<&str> {
    line.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .map(str::trim)
}

/// `config.go:288-299`: split on the first `=`, then trim and strip *any*
/// leading/trailing `"`/`'` characters from the value — not a real quoted-
/// string parse, which is exactly why `path = "run#1"` and `path = run#1`
/// (no quotes at all) read identically under v1.
fn split_field(line: &str) -> Option<(&str, String)> {
    let (key, raw_value) = line.split_once('=')?;
    let value = raw_value.trim().trim_matches(['"', '\'']).to_owned();
    Some((key.trim(), value))
}

/// The value portion of a `key = value` line, trimmed but with quotes intact —
/// used only for the ambiguity message's "as written" line, never for what
/// v1 actually reads (that always goes through [`split_field`]'s unquoting).
fn raw_value_text(line: &str) -> Option<String> {
    let (_, raw_value) = line.split_once('=')?;
    Some(raw_value.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "version = 1\n[remotes.default]\nbackend = \"myremote\"\n";

    #[test]
    fn scans_a_minimal_valid_config() {
        let cfg = scan(MINIMAL).unwrap();
        assert_eq!(
            cfg.remotes
                .get("default")
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .v1_reading,
            "myremote"
        );
    }

    #[test]
    fn strips_the_first_hash_anywhere_including_inside_quotes() {
        // contract-spec §6.3's exact dangerous example.
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let cfg = scan(text).unwrap();
        let path = cfg.remotes.get("default").unwrap().path.as_ref().unwrap();
        assert_eq!(path.v1_reading, "datasets/run");
        assert_eq!(path.as_written, "\"datasets/run#1\"");
    }

    #[test]
    fn a_genuine_trailing_comment_does_not_affect_the_value() {
        let text = "version = 1\n[remotes.default]\nbackend = \"myremote\"  # a comment\n";
        let cfg = scan(text).unwrap();
        assert_eq!(
            cfg.remotes
                .get("default")
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .v1_reading,
            "myremote"
        );
    }

    #[test]
    fn unquoted_values_are_accepted_same_as_quoted() {
        // The permissiveness that lets v1 read "TOML fails, v1 succeeds" inputs
        // (contract-spec §6.5 row 3): no real quoted-string parsing exists, so
        // an entirely unquoted value is just as legal as a quoted one.
        let text = "version = 1\n[remotes.default]\nbackend = myremote\n";
        let cfg = scan(text).unwrap();
        assert_eq!(
            cfg.remotes
                .get("default")
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .v1_reading,
            "myremote"
        );
    }

    #[test]
    fn rejects_missing_version() {
        assert!(matches!(
            scan("[remotes.default]\nbackend = \"m\"\n"),
            Err(LegacyScanError::MissingVersion)
        ));
    }

    #[test]
    fn rejects_wrong_version() {
        let text = "version = 2\n[remotes.default]\nbackend = \"m\"\n";
        assert!(matches!(
            scan(text),
            Err(LegacyScanError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn rejects_an_empty_remote_name() {
        assert!(matches!(
            scan("version = 1\n[remotes.]\nbackend = \"m\"\n"),
            Err(LegacyScanError::EmptyRemoteName { .. })
        ));
    }

    #[test]
    fn rejects_a_remote_missing_backend() {
        assert!(matches!(
            scan("version = 1\n[remotes.default]\npath = \"x\"\n"),
            Err(LegacyScanError::RemoteMissingBackend { .. })
        ));
    }

    #[test]
    fn rejects_a_cache_section() {
        assert!(matches!(
            scan("version = 1\n[cache]\npath = \"x\"\n"),
            Err(LegacyScanError::CacheNotAllowed)
        ));
        assert!(matches!(
            scan("version = 1\n[cache.local]\npath = \"x\"\n"),
            Err(LegacyScanError::CacheNotAllowed)
        ));
    }

    #[test]
    fn rejects_unknown_sections_and_fields() {
        assert!(matches!(
            scan("version = 1\n[bogus]\nx = 1\n"),
            Err(LegacyScanError::UnknownSection { .. })
        ));
        assert!(matches!(
            scan("version = 1\nbogus = 1\n[remotes.default]\nbackend=\"m\"\n"),
            Err(LegacyScanError::UnknownTopLevelField { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[settings]\nbogus = 1\n[remotes.default]\nbackend=\"m\"\n"),
            Err(LegacyScanError::UnknownSettingsField { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\nbogus=1\n"),
            Err(LegacyScanError::UnknownRemoteField { .. })
        ));
    }

    #[test]
    fn rejects_negative_n_jobs() {
        let text = "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nn_jobs = -1\n";
        assert!(matches!(scan(text), Err(LegacyScanError::NJobsNegative)));
    }

    #[test]
    fn accepts_negative_retry_max() {
        // contract-spec §6.2 only lists n_jobs's sign as a rejection rule;
        // retry_max is checked for "non-integer" only. Matching that
        // omission exactly, not tightening it, keeps this parser a faithful
        // oracle for what v1 actually accepts.
        let text = "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nretry_max = -1\n";
        assert_eq!(scan(text).unwrap().settings.retry_max, Some(-1));
    }

    #[test]
    fn rejects_non_integer_n_jobs_and_retry_max() {
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nn_jobs = \"x\"\n"),
            Err(LegacyScanError::NJobsNotAnInteger { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nretry_max = \"x\"\n"),
            Err(LegacyScanError::RetryMaxNotAnInteger { .. })
        ));
    }

    #[test]
    fn rejects_an_unsupported_algorithm() {
        let text =
            "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nalgorithm = \"md5\"\n";
        assert!(matches!(
            scan(text),
            Err(LegacyScanError::UnsupportedAlgorithm { .. })
        ));
    }
}
