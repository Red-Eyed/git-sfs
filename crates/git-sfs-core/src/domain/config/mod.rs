//! `.git-sfs/config.toml`: schema, validation, and the dual-parser divergence
//! check.
//!
//! git-sfs accepts TOML, but also detects configs whose meaning would differ
//! under the older compatibility scanner. The dangerous case is not a parse
//! failure on either side; it is two different values that both parse fine,
//! such as a quoted string containing `#`.
//!
//! `parse_and_validate` is the only public entry point; everything else here
//! is a detail of how the two readings are produced and compared.

mod compat_scanner;

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Deserialize;
use thiserror::Error;

use compat_scanner::{CompatConfig, StringField};

use super::remote::RemoteName;

/// A parsed, validated `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `[remotes.<name>]` entries, keyed by their validated name.
    pub remotes: BTreeMap<RemoteName, RemoteConfig>,
    /// The `[settings]` table, fully defaulted.
    pub settings: Settings,
}

/// One `[remotes.<name>]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    /// The rclone remote name, as defined in the user's rclone config.
    pub backend: String,
    /// The path within that backend. Absent means the backend's root.
    pub path: Option<String>,
    /// An rclone config file path, relative to `.git-sfs` if not absolute.
    pub rclone_config_path: Option<Utf8PathBuf>,
}

/// The `[settings]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Always `"sha256"` once validated. Kept as a field rather than collapsed
    /// away because the schema still names it explicitly and a future minor
    /// version could widen it.
    pub algorithm: String,
    /// `n_jobs` in the file. `0` means auto.
    pub jobs: u32,
    /// Retries per rclone call on transient failures.
    pub retry_max: Option<i64>,
    /// Fail fast if the installed rclone is older than this.
    pub min_rclone_version: Option<String>,
    /// Fail fast if git-sfs itself is older than this.
    pub min_git_sfs_version: Option<String>,
}

/// The default `.git-sfs/config.toml` template written by `git-sfs init`.
///
/// The template must parse under the strict TOML reader and the compatibility
/// scanner without ambiguity.
pub const DEFAULT_TEMPLATE: &str = r#"# git-sfs project config. Commit this file to Git.
# Do not put local cache paths, secrets, tokens, or machine-specific paths here.

version = 1

# The default remote is used by git-sfs push and git-sfs pull when no remote is named.
# "backend" must match a remote name defined in your rclone config.
[remotes.default]
backend = "YOUR_RCLONE_REMOTE"   # replace with a remote name from your rclone config
path = "your/remote/path"        # replace with the path within that remote

[settings]
# Only sha256 is supported.
algorithm = "sha256"
# Optional: cap parallel work for push, pull, verify, add, and import.
# 0 means auto.
n_jobs = 0
# Optional: retries for each rclone call on transient failures. Default 3.
# retry_max = 3
# Optional: fail fast if the installed rclone is older than this version.
# min_rclone_version = "1.67.0"
# Optional: fail fast if git-sfs itself is older than this version.
# min_git_sfs_version = "1.6.0"
"#;

/// Why a config failed to parse or validate.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Neither reading was usable: both parsers failed, or the strict reading
    /// succeeded while the compatibility scanner failed. The second case is
    /// deliberately treated as an error too, because a config should remain
    /// understandable to supported compatibility paths.
    #[error("invalid config: {0}")]
    Invalid(String),
    /// Both readings succeeded but disagree on one field's value. The
    /// `your objects are here` line tells the user which reading preserves the
    /// existing remote object location.
    #[error(
        "error: .git-sfs/config.toml: {field} is ambiguous\n\n  \
         as written:          {as_written}\n  \
         compatibility read:   {compat_reading}      ← your objects are here\n  \
         strict TOML reads:    {toml_reading}\n\n  \
         Change the line to make it unambiguous:\n      \
         {field_name} = \"{compat_reading}\""
    )]
    Ambiguous {
        /// The dotted path to the field, e.g. `remotes.default.path`.
        field: String,
        /// The bare key as it appears on its own line, e.g. `path`.
        field_name: String,
        /// The value's raw text in the file, quotes included.
        as_written: String,
        /// What the compatibility scanner reads.
        compat_reading: String,
        /// What strict TOML reads.
        toml_reading: String,
    },
}

/// Parses and validates `text` as a `config.toml`, running the compatibility
/// scanner and real TOML reader in parallel.
///
/// # Errors
///
/// Returns [`ConfigError`] if the config is invalid under the closed schema,
/// or if the two readings disagree ([`ConfigError::Ambiguous`]).
pub fn parse_and_validate(text: &str) -> Result<Config, ConfigError> {
    let compat_result = compat_scanner::scan(text);
    let strict_result = parse_raw(text).and_then(|raw| Ok((build_config(&raw)?, raw)));

    match (compat_result, strict_result) {
        (Ok(compat), Ok((config, raw))) => match find_divergence(&compat, &raw) {
            Some(err) => Err(err),
            None => Ok(config),
        },
        // Fall back to the compatibility reading only when TOML grammar rejects
        // the file outright, for example for an unquoted bare value. Semantic
        // rejections such as empty remote names or unsupported algorithms still
        // fail, even if comment truncation happens to produce a valid-looking
        // compatibility reading.
        (Ok(compat), Err(StrictTomlError::Toml(_))) => Ok(compat_to_config(compat)),
        (Ok(_), Err(strict_err)) => Err(ConfigError::Invalid(strict_err.to_string())),
        (Err(compat_err), Ok(_)) => Err(ConfigError::Invalid(format!(
            "compatibility scanner could not read this config ({compat_err})"
        ))),
        (Err(compat_err), Err(strict_err)) => Err(ConfigError::Invalid(format!(
            "{strict_err} (compatibility reading also failed: {compat_err})"
        ))),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: i64,
    #[serde(default)]
    remotes: BTreeMap<String, RawRemoteConfig>,
    #[serde(default)]
    settings: RawSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRemoteConfig {
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    config: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawSettings {
    algorithm: Option<String>,
    // Deserializing a negative TOML integer into `u32` fails at the `toml`
    // crate boundary, so there is no runtime `< 0` check to forget. `retry_max`
    // stays signed because negative values are not rejected by the schema.
    n_jobs: Option<u32>,
    retry_max: Option<i64>,
    min_rclone_version: Option<String>,
    min_git_sfs_version: Option<String>,
}

/// Why the strict TOML reading failed. Kept separate from
/// [`compat_scanner::CompatScanError`] since the two parsers reject different
/// things for different reasons and nothing downstream needs to unify them.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
enum StrictTomlError {
    #[error("{0}")]
    Toml(String),
    #[error(".git-sfs/config.toml must not contain local cache configuration")]
    CacheNotAllowed,
    #[error("remote name must not be empty")]
    EmptyRemoteName,
    #[error("unsupported .git-sfs/config.toml version {value}")]
    UnsupportedVersion { value: i64 },
    #[error("unsupported hash algorithm {value:?}")]
    UnsupportedAlgorithm { value: String },
    #[error("remote {name:?} requires backend")]
    RemoteMissingBackend { name: String },
}

/// Structural parsing only: the closed schema (`deny_unknown_fields` at every
/// level) plus the two checks real TOML's own type system cannot express —
/// rejecting `cache`/`[cache.*]` before it would otherwise fall out as a
/// generic "unknown field", and rejecting an explicitly-empty remote name
/// (`[remotes.""]` is syntactically legal TOML).
fn parse_raw(text: &str) -> Result<RawConfig, StrictTomlError> {
    // A whole document is a table, not a value -- `toml::Value::from_str`
    // parses a single value expression and fails on anything containing a
    // top-level `key = value` line.
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| StrictTomlError::Toml(e.to_string()))?;
    if table.contains_key("cache") {
        return Err(StrictTomlError::CacheNotAllowed);
    }
    if table
        .get("remotes")
        .and_then(toml::Value::as_table)
        .is_some_and(|t| t.contains_key(""))
    {
        return Err(StrictTomlError::EmptyRemoteName);
    }
    toml::from_str(text).map_err(|e| StrictTomlError::Toml(e.to_string()))
}

/// Semantic validation and defaulting over an already structurally-valid
/// [`RawConfig`].
fn build_config(raw: &RawConfig) -> Result<Config, StrictTomlError> {
    if raw.version != 1 {
        return Err(StrictTomlError::UnsupportedVersion { value: raw.version });
    }

    let algorithm = raw
        .settings
        .algorithm
        .clone()
        .unwrap_or_else(|| "sha256".to_owned());
    if algorithm != "sha256" {
        return Err(StrictTomlError::UnsupportedAlgorithm { value: algorithm });
    }

    let mut remotes = BTreeMap::new();
    for (name, r) in &raw.remotes {
        let backend = r
            .backend
            .as_ref()
            .filter(|b| !b.is_empty())
            .ok_or_else(|| StrictTomlError::RemoteMissingBackend { name: name.clone() })?;
        let remote_name = RemoteName::parse(name.clone()).expect("checked non-empty by parse_raw");
        remotes.insert(
            remote_name,
            RemoteConfig {
                backend: backend.clone(),
                path: r.path.clone(),
                rclone_config_path: r.config.clone().map(Utf8PathBuf::from),
            },
        );
    }

    Ok(Config {
        remotes,
        settings: Settings {
            algorithm,
            jobs: raw.settings.n_jobs.unwrap_or(0),
            retry_max: raw.settings.retry_max,
            min_rclone_version: raw.settings.min_rclone_version.clone(),
            min_git_sfs_version: raw.settings.min_git_sfs_version.clone(),
        },
    })
}

/// Converts an already-validated [`CompatConfig`] into the public shape, for
/// the case where strict TOML grammar fails but the compatibility scanner
/// succeeds. Infallible because the scanner already enforced every rule
/// `build_config` would otherwise re-check.
fn compat_to_config(compat: CompatConfig) -> Config {
    let remotes = compat
        .remotes
        .into_iter()
        .map(|(name, r)| {
            let remote_name = RemoteName::parse(name).expect("scan() rejects empty remote names");
            let config = RemoteConfig {
                backend: r.backend.expect("scan() requires backend").compat_reading,
                path: r.path.map(|f| f.compat_reading),
                rclone_config_path: r.config.map(|f| Utf8PathBuf::from(f.compat_reading)),
            };
            (remote_name, config)
        })
        .collect();

    Config {
        remotes,
        settings: Settings {
            algorithm: compat
                .settings
                .algorithm
                .map(|f| f.compat_reading)
                .unwrap_or_else(|| "sha256".to_owned()),
            jobs: compat
                .settings
                .n_jobs
                .unwrap_or(0)
                .try_into()
                .expect("scan() rejects negative n_jobs"),
            retry_max: compat.settings.retry_max,
            min_rclone_version: compat.settings.min_rclone_version.map(|f| f.compat_reading),
            min_git_sfs_version: compat
                .settings
                .min_git_sfs_version
                .map(|f| f.compat_reading),
        },
    }
}

/// Looks for the first field where the two readings disagree.
///
/// Scoped to string-valued fields deliberately: quoting and `#`-in-string
/// handling only affect string values. Numeric fields (`n_jobs`, `retry_max`,
/// `version`) are bare, unquoted integer literals under both parsers with no
/// comment-vs-quote ambiguity possible, so there is nothing to compare there.
///
/// A field absent from the file is `None` on both sides and never compared —
/// only a field genuinely present is a candidate for disagreement, which is
/// why this takes the compatibility scanner's pre-defaulting [`CompatConfig`]
/// against strict TOML's pre-defaulting [`RawConfig`] rather than either side's
/// already-defaulted [`Config`]: comparing post-default values would read
/// "field omitted entirely" as a disagreement against whatever the default
/// happens to be.
fn find_divergence(compat: &CompatConfig, raw: &RawConfig) -> Option<ConfigError> {
    let string_field =
        |dotted: &str, bare: &str, compat: Option<&StringField>, toml: Option<&String>| {
            let compat = compat?;
            let toml = toml?;
            (compat.compat_reading != *toml).then(|| ConfigError::Ambiguous {
                field: dotted.to_owned(),
                field_name: bare.to_owned(),
                as_written: compat.as_written.clone(),
                compat_reading: compat.compat_reading.clone(),
                toml_reading: toml.clone(),
            })
        };

    if let Some(err) = string_field(
        "settings.algorithm",
        "algorithm",
        compat.settings.algorithm.as_ref(),
        raw.settings.algorithm.as_ref(),
    ) {
        return Some(err);
    }
    if let Some(err) = string_field(
        "settings.min_rclone_version",
        "min_rclone_version",
        compat.settings.min_rclone_version.as_ref(),
        raw.settings.min_rclone_version.as_ref(),
    ) {
        return Some(err);
    }
    if let Some(err) = string_field(
        "settings.min_git_sfs_version",
        "min_git_sfs_version",
        compat.settings.min_git_sfs_version.as_ref(),
        raw.settings.min_git_sfs_version.as_ref(),
    ) {
        return Some(err);
    }

    for (name, compat_remote) in &compat.remotes {
        let Some(raw_remote) = raw.remotes.get(name) else {
            continue;
        };
        // `backend` is required on both sides by the time either parser
        // succeeds, so unlike the optional fields below it is always present
        // to compare rather than needing the `Option`-aware helper.
        if let (Some(compat_backend), Some(toml_backend)) =
            (&compat_remote.backend, &raw_remote.backend)
            && compat_backend.compat_reading != *toml_backend
        {
            return Some(ConfigError::Ambiguous {
                field: format!("remotes.{name}.backend"),
                field_name: "backend".to_owned(),
                as_written: compat_backend.as_written.clone(),
                compat_reading: compat_backend.compat_reading.clone(),
                toml_reading: toml_backend.clone(),
            });
        }
        if let Some(err) = string_field(
            &format!("remotes.{name}.path"),
            "path",
            compat_remote.path.as_ref(),
            raw_remote.path.as_ref(),
        ) {
            return Some(err);
        }
        if let Some(err) = string_field(
            &format!("remotes.{name}.config"),
            "config",
            compat_remote.config.as_ref(),
            raw_remote.config.as_ref(),
        ) {
            return Some(err);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_template_parses_and_does_not_trip_its_own_ambiguity_check() {
        let config = parse_and_validate(DEFAULT_TEMPLATE).unwrap();
        assert_eq!(config.settings.algorithm, "sha256");
        assert_eq!(config.settings.jobs, 0);
        let default = config.remotes.get("default").unwrap();
        assert_eq!(default.backend, "YOUR_RCLONE_REMOTE");
        assert_eq!(default.path.as_deref(), Some("your/remote/path"));
        assert_eq!(default.rclone_config_path, None);
    }

    #[test]
    fn a_minimal_config_parses_with_defaults() {
        let text = "version = 1\n[remotes.default]\nbackend = \"myremote\"\n";
        let config = parse_and_validate(text).unwrap();
        assert_eq!(config.settings.algorithm, "sha256");
        assert_eq!(config.settings.jobs, 0);
        assert!(config.remotes.get("default").unwrap().path.is_none());
    }

    /// A `#` inside a quoted string is a different remote path under each
    /// parser, and both readings succeed.
    #[test]
    fn a_hash_inside_a_quoted_string_is_reported_as_ambiguous_not_silently_resolved() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let err = parse_and_validate(text).unwrap_err();
        let ConfigError::Ambiguous {
            field,
            compat_reading,
            toml_reading,
            as_written,
            field_name,
        } = err
        else {
            panic!("expected Ambiguous, got {err:?}");
        };
        assert_eq!(field, "remotes.default.path");
        assert_eq!(field_name, "path");
        assert_eq!(compat_reading, "datasets/run");
        assert_eq!(toml_reading, "datasets/run#1");
        assert_eq!(as_written, "\"datasets/run#1\"");
    }

    /// The ambiguity message must preserve the line that tells the user which
    /// reading points at existing objects.
    #[test]
    fn the_ambiguity_message_preserves_the_recovery_template() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let err = parse_and_validate(text).unwrap_err();
        assert_eq!(
            err.to_string(),
            "error: .git-sfs/config.toml: remotes.default.path is ambiguous\n\
             \n  \
             as written:          \"datasets/run#1\"\n  \
             compatibility read:   datasets/run      ← your objects are here\n  \
             strict TOML reads:    datasets/run#1\n\
             \n  \
             Change the line to make it unambiguous:\n      \
             path = \"datasets/run\""
        );
    }

    #[test]
    fn a_genuine_trailing_comment_is_not_ambiguous() {
        // Both parsers land on the same `#` -- it truly is a comment, not
        // divergent quoting -- so this must parse cleanly, not error.
        let text = "version = 1\n[remotes.default]\nbackend = \"myremote\"  # prod\n";
        let config = parse_and_validate(text).unwrap();
        assert_eq!(config.remotes.get("default").unwrap().backend, "myremote");
    }

    #[test]
    fn an_entirely_unquoted_value_is_compatibility_only_syntax() {
        // TOML rejects bare words, but the compatibility scanner accepts them.
        let text = "version = 1\n[remotes.default]\nbackend = myremote\n";
        let config = parse_and_validate(text).unwrap();
        assert_eq!(config.remotes.get("default").unwrap().backend, "myremote");
    }

    #[test]
    fn rejects_a_config_neither_parser_can_read() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\nbogus = 1\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_a_config_the_compatibility_scanner_cannot_read_even_though_toml_can() {
        // A genuine TOML array: strict TOML fails to deserialize it into the
        // scalar `retry_max: Option<i64>` field, and the compatibility scanner
        // also rejects it as non-integer. Both fail rather than silently
        // trusting either reading.
        let text =
            "version = 1\n[remotes.default]\nbackend = \"m\"\n[settings]\nretry_max = [1, 2, 3]\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_unknown_top_level_fields_via_the_closed_schema() {
        let text = "version = 1\nbogus = 1\n[remotes.default]\nbackend = \"m\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_a_cache_section() {
        let text = "version = 1\n[cache]\npath = \"x\"\n[remotes.default]\nbackend = \"m\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_negative_n_jobs_structurally() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\n[settings]\nn_jobs = -1\n";
        // The `u32` field type itself is the enforcement -- deserializing
        // fails before any semantic check would even run.
        assert!(parse_raw(text).is_err());
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_wrong_version() {
        let text = "version = 2\n[remotes.default]\nbackend = \"m\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_an_empty_remote_name_written_as_an_explicit_toml_key() {
        // `[remotes.""]` is syntactically legal TOML, so `parse_raw` needs a
        // dedicated semantic check for it.
        let text = "version = 1\n[remotes.\"\"]\nbackend = \"m\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_a_remote_missing_backend() {
        let text = "version = 1\n[remotes.default]\npath = \"x\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }

    /// A semantic rejection must propagate even when the compatibility scanner
    /// "succeeds" only because comment-truncation happens to land on a value
    /// that still looks valid.
    #[test]
    fn a_semantic_rejection_is_not_masked_by_compatibility_truncation() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\n[settings]\nalgorithm = \"sha256#not-sha256\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }
}
