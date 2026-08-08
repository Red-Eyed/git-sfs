//! Compatibility scanner for older git-sfs config syntax.
//!
//! This is the deliberate exception to "use a crate, don't reinvent the
//! parser": no generic TOML implementation replicates this scanner's specific
//! quirks, such as comment-stripping that does not know about quotes and
//! quote-trimming that does not know about escapes. Reproducing those quirks is
//! how git-sfs detects configs that strict TOML would read differently.
//!
//! Two things per string field are captured, not one:
//! - `compat_reading`: what the compatibility pipeline computes after
//!   comment-stripping and quote-trimming.
//! - `as_written`: the trimmed value text from the *original*, un-stripped
//!   line, used to show the value as the user actually typed it.

use std::collections::BTreeMap;

use thiserror::Error;

/// A string-valued field, with both compatibility output and original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StringField {
    pub compat_reading: String,
    pub as_written: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompatRemote {
    pub backend: Option<StringField>,
    pub path: Option<StringField>,
    pub config: Option<StringField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompatSettings {
    pub algorithm: Option<StringField>,
    pub n_jobs: Option<i64>,
    pub retry_max: Option<i64>,
    pub min_rclone_version: Option<StringField>,
    pub min_git_sfs_version: Option<StringField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompatConfig {
    pub remotes: BTreeMap<String, CompatRemote>,
    pub settings: CompatSettings,
}

/// Every way the compatibility scanner can reject a file.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum CompatScanError {
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

/// Runs the compatibility scan-and-validate pipeline over `text`.
pub(super) fn scan(text: &str) -> Result<CompatConfig, CompatScanError> {
    let mut cfg = CompatConfig::default();
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
                    return Err(CompatScanError::EmptyRemoteName {
                        name: name.to_owned(),
                    });
                }
                section = Section::Remotes;
                current_remote = Some(remote.to_owned());
                cfg.remotes.entry(remote.to_owned()).or_default();
            } else if name == "cache" || name.starts_with("cache.") {
                return Err(CompatScanError::CacheNotAllowed);
            } else {
                return Err(CompatScanError::UnknownSection {
                    name: name.to_owned(),
                });
            }
            continue;
        }

        let Some((key, compat_value)) = split_field(line) else {
            return Err(CompatScanError::InvalidLine {
                line: line.to_owned(),
            });
        };
        // `as_written` is the value text from the original, non-comment-
        // stripped line, quotes intact, so ambiguity messages can show the
        // value exactly as the user typed it.
        let as_written = raw_value_text(raw_line.trim()).unwrap_or_else(|| compat_value.clone());
        let field = |compat_reading: String| StringField {
            compat_reading,
            as_written,
        };

        match section {
            Section::TopLevel if key == "version" => version_field = Some(compat_value),
            Section::TopLevel if key == "cache" => return Err(CompatScanError::CacheNotAllowed),
            Section::TopLevel => {
                return Err(CompatScanError::UnknownTopLevelField {
                    key: key.to_owned(),
                });
            }

            Section::Settings => match key {
                "algorithm" => cfg.settings.algorithm = Some(field(compat_value)),
                "n_jobs" => {
                    cfg.settings.n_jobs = Some(compat_value.parse().map_err(|_| {
                        CompatScanError::NJobsNotAnInteger {
                            value: compat_value.clone(),
                        }
                    })?);
                }
                "retry_max" => {
                    cfg.settings.retry_max = Some(compat_value.parse().map_err(|_| {
                        CompatScanError::RetryMaxNotAnInteger {
                            value: compat_value.clone(),
                        }
                    })?);
                }
                "min_rclone_version" => cfg.settings.min_rclone_version = Some(field(compat_value)),
                "min_git_sfs_version" => {
                    cfg.settings.min_git_sfs_version = Some(field(compat_value))
                }
                _ => {
                    return Err(CompatScanError::UnknownSettingsField {
                        key: key.to_owned(),
                    });
                }
            },

            Section::Remotes => {
                // Structurally unreachable: entering `Remotes` and setting
                // `current_remote` happen together at the section header, so
                // no key/value line can see one without the other. Kept as a
                // guard against future edits to the state machine.
                let Some(remote) = &current_remote else {
                    return Err(CompatScanError::FieldBeforeRemoteName {
                        key: key.to_owned(),
                    });
                };
                let entry = cfg
                    .remotes
                    .get_mut(remote)
                    .expect("remote entry created at its section header");
                match key {
                    "backend" => entry.backend = Some(field(compat_value)),
                    "path" => entry.path = Some(field(compat_value)),
                    "config" => entry.config = Some(field(compat_value)),
                    _ => {
                        return Err(CompatScanError::UnknownRemoteField {
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
            return Err(CompatScanError::UnsupportedVersion {
                value: other.to_owned(),
            });
        }
        None => return Err(CompatScanError::MissingVersion),
    }

    if cfg.settings.n_jobs.is_some_and(|n| n < 0) {
        return Err(CompatScanError::NJobsNegative);
    }
    let algorithm = cfg
        .settings
        .algorithm
        .as_ref()
        .map(|f| f.compat_reading.as_str());
    if let Some(algorithm) = algorithm
        && algorithm != "sha256"
    {
        return Err(CompatScanError::UnsupportedAlgorithm {
            value: algorithm.to_owned(),
        });
    }
    for (name, remote) in &cfg.remotes {
        if remote.backend.is_none() {
            return Err(CompatScanError::RemoteMissingBackend { name: name.clone() });
        }
    }

    Ok(cfg)
}

/// The first `#` anywhere in the line starts a comment. This is deliberately
/// quote-unaware so it matches the compatibility scanner's historical syntax.
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// A trimmed line wrapped in `[` and `]` is a section header.
fn section_header(line: &str) -> Option<&str> {
    line.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .map(str::trim)
}

/// Split on the first `=`, then trim and strip *any* leading/trailing `"`/`'`
/// characters from the value. This is not a real quoted-string parse, which is
/// why `path = "run#1"` and `path = run#1` read identically here.
fn split_field(line: &str) -> Option<(&str, String)> {
    let (key, raw_value) = line.split_once('=')?;
    let value = raw_value.trim().trim_matches(['"', '\'']).to_owned();
    Some((key.trim(), value))
}

/// The value portion of a `key = value` line, trimmed but with quotes intact —
/// used only for the ambiguity message's "as written" line, never for what
/// the compatibility scanner reads (that always goes through
/// [`split_field`]'s unquoting).
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
                .compat_reading,
            "myremote"
        );
    }

    #[test]
    fn strips_the_first_hash_anywhere_including_inside_quotes() {
        // The dangerous case: the `#` is data to TOML but a comment here.
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let cfg = scan(text).unwrap();
        let path = cfg.remotes.get("default").unwrap().path.as_ref().unwrap();
        assert_eq!(path.compat_reading, "datasets/run");
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
                .compat_reading,
            "myremote"
        );
    }

    #[test]
    fn unquoted_values_are_accepted_same_as_quoted() {
        // No real quoted-string parsing exists, so an entirely unquoted value
        // is just as legal as a quoted one.
        let text = "version = 1\n[remotes.default]\nbackend = myremote\n";
        let cfg = scan(text).unwrap();
        assert_eq!(
            cfg.remotes
                .get("default")
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .compat_reading,
            "myremote"
        );
    }

    #[test]
    fn rejects_missing_version() {
        assert!(matches!(
            scan("[remotes.default]\nbackend = \"m\"\n"),
            Err(CompatScanError::MissingVersion)
        ));
    }

    #[test]
    fn rejects_wrong_version() {
        let text = "version = 2\n[remotes.default]\nbackend = \"m\"\n";
        assert!(matches!(
            scan(text),
            Err(CompatScanError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn rejects_an_empty_remote_name() {
        assert!(matches!(
            scan("version = 1\n[remotes.]\nbackend = \"m\"\n"),
            Err(CompatScanError::EmptyRemoteName { .. })
        ));
    }

    #[test]
    fn rejects_a_remote_missing_backend() {
        assert!(matches!(
            scan("version = 1\n[remotes.default]\npath = \"x\"\n"),
            Err(CompatScanError::RemoteMissingBackend { .. })
        ));
    }

    #[test]
    fn rejects_a_cache_section() {
        assert!(matches!(
            scan("version = 1\n[cache]\npath = \"x\"\n"),
            Err(CompatScanError::CacheNotAllowed)
        ));
        assert!(matches!(
            scan("version = 1\n[cache.local]\npath = \"x\"\n"),
            Err(CompatScanError::CacheNotAllowed)
        ));
    }

    #[test]
    fn rejects_unknown_sections_and_fields() {
        assert!(matches!(
            scan("version = 1\n[bogus]\nx = 1\n"),
            Err(CompatScanError::UnknownSection { .. })
        ));
        assert!(matches!(
            scan("version = 1\nbogus = 1\n[remotes.default]\nbackend=\"m\"\n"),
            Err(CompatScanError::UnknownTopLevelField { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[settings]\nbogus = 1\n[remotes.default]\nbackend=\"m\"\n"),
            Err(CompatScanError::UnknownSettingsField { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\nbogus=1\n"),
            Err(CompatScanError::UnknownRemoteField { .. })
        ));
    }

    #[test]
    fn rejects_negative_n_jobs() {
        let text = "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nn_jobs = -1\n";
        assert!(matches!(scan(text), Err(CompatScanError::NJobsNegative)));
    }

    #[test]
    fn accepts_negative_retry_max() {
        // `retry_max` is checked for "integer" only; negative retry counts are
        // allowed by the compatibility grammar.
        let text = "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nretry_max = -1\n";
        assert_eq!(scan(text).unwrap().settings.retry_max, Some(-1));
    }

    #[test]
    fn rejects_non_integer_n_jobs_and_retry_max() {
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nn_jobs = \"x\"\n"),
            Err(CompatScanError::NJobsNotAnInteger { .. })
        ));
        assert!(matches!(
            scan("version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nretry_max = \"x\"\n"),
            Err(CompatScanError::RetryMaxNotAnInteger { .. })
        ));
    }

    #[test]
    fn rejects_an_unsupported_algorithm() {
        let text =
            "version = 1\n[remotes.default]\nbackend=\"m\"\n[settings]\nalgorithm = \"md5\"\n";
        assert!(matches!(
            scan(text),
            Err(CompatScanError::UnsupportedAlgorithm { .. })
        ));
    }
}
