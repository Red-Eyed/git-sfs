//! `.git-sfs/config.toml`: schema, validation, and the dual-parser divergence
//! check.
//!
//! contract-spec §6. This is, in the rewrite plan's own words, **"the
//! highest-risk item for the rewrite"** (§6.3): v1's parser is a hand-rolled
//! line scanner, not real TOML, and it disagrees with real TOML on quoted
//! strings containing `#`. Both readings therefore run on every parse
//! ([`legacy_scanner`] and this module's own `serde`+`toml` deserialization),
//! and a disagreement between them is reported rather than silently resolved
//! either way — §6.5 is explicit that a fallback chain (try TOML, fall back to
//! v1 on error) does not work, because the dangerous case is not a parse
//! failure on either side, it is two different values that both parse fine.
//!
//! `parse_and_validate` is the only public entry point; everything else here
//! is a detail of how the two readings are produced and compared.

mod legacy_scanner;

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Deserialize;
use thiserror::Error;

use legacy_scanner::{LegacyConfig, StringField};

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
    /// Always `"sha256"` once validated — contract-spec §6.2 accepts no other
    /// value. Kept as a field rather than collapsed away because the schema
    /// still names it explicitly and a future minor version could widen it.
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

/// Why a config failed to parse or validate.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Neither reading was usable: both parsers failed, or the strict
    /// reading succeeded while the legacy one failed. The second case is
    /// deliberately treated as an error too, not a silent accept — contract-
    /// spec §7c requires v1 to remain able to operate any repo v2 has
    /// touched, and a config only the `toml` crate can read would break that
    /// the moment someone downgrades.
    #[error("invalid config: {0}")]
    Invalid(String),
    /// contract-spec §6.5's exact scenario: both readings succeeded but
    /// disagree on one field's value. The message format is specified, not
    /// left to the implementer — the `your objects are here` line is what
    /// lets the user tell which reading is real without guessing.
    #[error(
        "error: .git-sfs/config.toml: {field} is ambiguous\n\n  \
         as written:          {as_written}\n  \
         git-sfs 1.x read:     {v1_reading}      ← your objects are here\n  \
         strict TOML reads:    {toml_reading}\n\n  \
         Change the line to make it unambiguous:\n      \
         {field_name} = \"{v1_reading}\""
    )]
    Ambiguous {
        /// The dotted path to the field, e.g. `remotes.default.path`.
        field: String,
        /// The bare key as it appears on its own line, e.g. `path`.
        field_name: String,
        /// The value's raw text in the file, quotes included.
        as_written: String,
        /// What v1's scanner reads.
        v1_reading: String,
        /// What strict TOML reads.
        toml_reading: String,
    },
}

/// Parses and validates `text` as a `config.toml`, running both v1's line
/// scanner and real TOML in parallel and reconciling them per contract-spec
/// §6.5.
///
/// # Errors
///
/// Returns [`ConfigError`] if the config is invalid under the closed schema
/// (§6.2), or if the two readings disagree ([`ConfigError::Ambiguous`]).
pub fn parse_and_validate(text: &str) -> Result<Config, ConfigError> {
    let legacy_result = legacy_scanner::scan(text);
    let strict_result = parse_raw(text).and_then(|raw| Ok((build_config(&raw)?, raw)));

    match (legacy_result, strict_result) {
        (Ok(legacy), Ok((config, raw))) => match find_divergence(&legacy, &raw) {
            Some(err) => Err(err),
            None => Ok(config),
        },
        // §6.5 row 3, "TOML fails, v1 succeeds -> use v1's reading" -- but
        // only for a genuine TOML *grammar* rejection (`StrictTomlError::Toml`,
        // e.g. an unquoted bare value real TOML cannot parse at all). A
        // *semantic* rejection (empty remote name, missing backend, wrong
        // version, disallowed cache config, unsupported algorithm) is a
        // deliberate validation rule and must hold regardless of what the
        // legacy scanner made of the same text. This distinction matters even
        // when it looks redundant: `[remotes.""]` is syntactically valid TOML
        // naming a truly empty key, but v1's section-header handling never
        // strips quotes at all, so it reads the literal two-character text
        // `""` as a non-empty name and "succeeds" -- deferring to that
        // reading would silently accept a config no sane remote name should
        // be. The same gap can hide a real typo: `algorithm = "sha256#x"`
        // reads as invalid under strict TOML but as the valid-looking
        // `"sha256"` under v1's comment-unaware truncation, and only
        // propagating the semantic error surfaces that instead of silently
        // trusting v1's truncated (and coincidentally passing) reading.
        (Ok(legacy), Err(StrictTomlError::Toml(_))) => Ok(legacy_to_config(legacy)),
        (Ok(_), Err(strict_err)) => Err(ConfigError::Invalid(strict_err.to_string())),
        (Err(legacy_err), Ok(_)) => Err(ConfigError::Invalid(format!(
            "git-sfs 1.x could not read this config ({legacy_err}); refusing a config v1 cannot also operate on"
        ))),
        (Err(legacy_err), Err(strict_err)) => Err(ConfigError::Invalid(format!(
            "{strict_err} (v1 reading also failed: {legacy_err})"
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
    // A structural, not merely conventional, enforcement of contract-spec
    // §6.2's "n_jobs negative ... is an error": deserializing a negative
    // TOML integer into `u32` fails at the `toml` crate boundary, so there is
    // no runtime `< 0` check to forget. `retry_max` has no such rule (only
    // "non-integer" is specified), so it stays signed — see
    // `legacy_scanner`'s matching test for why that asymmetry is intentional.
    n_jobs: Option<u32>,
    retry_max: Option<i64>,
    min_rclone_version: Option<String>,
    min_git_sfs_version: Option<String>,
}

/// Why the strict TOML reading failed. Kept separate from
/// [`legacy_scanner::LegacyScanError`] since the two parsers reject different
/// things for different reasons and nothing downstream needs to unify them —
/// see [`ConfigError::Invalid`], which only ever renders one as a string.
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
/// (`[remotes.""]` is syntactically legal TOML, unlike v1's bare
/// `[remotes.]`, which is a syntax error under real TOML and needs no
/// separate check here).
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

/// Semantic validation and defaulting (contract-spec §6.2/§6.4) over an
/// already structurally-valid [`RawConfig`].
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

/// Converts an already-validated [`LegacyConfig`] into the public shape, for
/// the "TOML fails, v1 succeeds" row of contract-spec §6.5's table. Infallible
/// because `legacy_scanner::scan` already enforced every rule `build_config`
/// would otherwise re-check (version, algorithm, backend, non-empty names).
fn legacy_to_config(legacy: LegacyConfig) -> Config {
    let remotes = legacy
        .remotes
        .into_iter()
        .map(|(name, r)| {
            let remote_name = RemoteName::parse(name).expect("scan() rejects empty remote names");
            let config = RemoteConfig {
                backend: r.backend.expect("scan() requires backend").v1_reading,
                path: r.path.map(|f| f.v1_reading),
                rclone_config_path: r.config.map(|f| Utf8PathBuf::from(f.v1_reading)),
            };
            (remote_name, config)
        })
        .collect();

    Config {
        remotes,
        settings: Settings {
            algorithm: legacy
                .settings
                .algorithm
                .map(|f| f.v1_reading)
                .unwrap_or_else(|| "sha256".to_owned()),
            jobs: legacy
                .settings
                .n_jobs
                .unwrap_or(0)
                .try_into()
                .expect("scan() rejects negative n_jobs"),
            retry_max: legacy.settings.retry_max,
            min_rclone_version: legacy.settings.min_rclone_version.map(|f| f.v1_reading),
            min_git_sfs_version: legacy.settings.min_git_sfs_version.map(|f| f.v1_reading),
        },
    }
}

/// Looks for the first field where the two readings disagree.
///
/// Scoped to string-valued fields deliberately: contract-spec §6.3's
/// divergence is specifically about quoting and `#`-in-string handling, which
/// only string values are exposed to. Numeric fields (`n_jobs`, `retry_max`,
/// `version`) are bare, unquoted integer literals under both parsers with no
/// comment-vs-quote ambiguity possible, so there is nothing to compare there.
///
/// A field absent from the file is `None` on both sides and never compared —
/// only a field genuinely present is a candidate for disagreement, which is
/// why this takes v1's pre-defaulting `LegacyConfig` against strict TOML's
/// pre-defaulting [`RawConfig`] rather than either side's already-defaulted
/// [`Config`]: comparing post-default values would read "field omitted
/// entirely" as a disagreement against whatever the default happens to be.
fn find_divergence(legacy: &LegacyConfig, raw: &RawConfig) -> Option<ConfigError> {
    let string_field =
        |dotted: &str, bare: &str, legacy: Option<&StringField>, toml: Option<&String>| {
            let legacy = legacy?;
            let toml = toml?;
            (legacy.v1_reading != *toml).then(|| ConfigError::Ambiguous {
                field: dotted.to_owned(),
                field_name: bare.to_owned(),
                as_written: legacy.as_written.clone(),
                v1_reading: legacy.v1_reading.clone(),
                toml_reading: toml.clone(),
            })
        };

    if let Some(err) = string_field(
        "settings.algorithm",
        "algorithm",
        legacy.settings.algorithm.as_ref(),
        raw.settings.algorithm.as_ref(),
    ) {
        return Some(err);
    }
    if let Some(err) = string_field(
        "settings.min_rclone_version",
        "min_rclone_version",
        legacy.settings.min_rclone_version.as_ref(),
        raw.settings.min_rclone_version.as_ref(),
    ) {
        return Some(err);
    }
    if let Some(err) = string_field(
        "settings.min_git_sfs_version",
        "min_git_sfs_version",
        legacy.settings.min_git_sfs_version.as_ref(),
        raw.settings.min_git_sfs_version.as_ref(),
    ) {
        return Some(err);
    }

    for (name, legacy_remote) in &legacy.remotes {
        let Some(raw_remote) = raw.remotes.get(name) else {
            continue;
        };
        // `backend` is required on both sides by the time either parser
        // succeeds, so unlike the optional fields below it is always present
        // to compare rather than needing the `Option`-aware helper.
        if let (Some(legacy_backend), Some(toml_backend)) =
            (&legacy_remote.backend, &raw_remote.backend)
            && legacy_backend.v1_reading != *toml_backend
        {
            return Some(ConfigError::Ambiguous {
                field: format!("remotes.{name}.backend"),
                field_name: "backend".to_owned(),
                as_written: legacy_backend.as_written.clone(),
                v1_reading: legacy_backend.v1_reading.clone(),
                toml_reading: toml_backend.clone(),
            });
        }
        if let Some(err) = string_field(
            &format!("remotes.{name}.path"),
            "path",
            legacy_remote.path.as_ref(),
            raw_remote.path.as_ref(),
        ) {
            return Some(err);
        }
        if let Some(err) = string_field(
            &format!("remotes.{name}.config"),
            "config",
            legacy_remote.config.as_ref(),
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

    /// contract-spec §6.4: the template `init` writes must parse under the
    /// implementation's own validator and, under §6.5, identically under
    /// both parsers — a template that tripped its own ambiguity check would
    /// make `init` produce a repo no command could open. Copied verbatim from
    /// `internal/config/config.go`'s `defaultTOML`.
    const DEFAULT_TEMPLATE: &str = r#"# git-sfs project config. Commit this file to Git.
# Do not put local cache paths, secrets, tokens, or machine-specific paths here.

version = 1

# The default remote is used by git-sfs push and git-sfs pull when no remote is named.
# "backend" must match a remote name defined in your rclone config.
[remotes.default]
backend = "YOUR_RCLONE_REMOTE"   # replace with a remote name from your rclone config
path = "your/remote/path"        # replace with the path within that remote
# Relative paths are resolved from .git-sfs.
# Do not commit rclone configs that contain secrets or tokens.
config = "rclone.conf"

[settings]
# Only sha256 is supported in v1.
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

    #[test]
    fn the_default_template_parses_and_does_not_trip_its_own_ambiguity_check() {
        let config = parse_and_validate(DEFAULT_TEMPLATE).unwrap();
        assert_eq!(config.settings.algorithm, "sha256");
        assert_eq!(config.settings.jobs, 0);
        let default = config.remotes.get("default").unwrap();
        assert_eq!(default.backend, "YOUR_RCLONE_REMOTE");
        assert_eq!(default.path.as_deref(), Some("your/remote/path"));
        assert_eq!(
            default.rclone_config_path.as_deref(),
            Some(Utf8PathBuf::from("rclone.conf").as_path())
        );
    }

    #[test]
    fn a_minimal_config_parses_with_defaults() {
        let text = "version = 1\n[remotes.default]\nbackend = \"myremote\"\n";
        let config = parse_and_validate(text).unwrap();
        assert_eq!(config.settings.algorithm, "sha256");
        assert_eq!(config.settings.jobs, 0);
        assert!(config.remotes.get("default").unwrap().path.is_none());
    }

    /// contract-spec §6.3/§6.5's exact worked example: a `#` inside a quoted
    /// string is a different remote path under each parser, and both readings
    /// succeed -- the specific case a fallback chain cannot catch.
    #[test]
    fn a_hash_inside_a_quoted_string_is_reported_as_ambiguous_not_silently_resolved() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let err = parse_and_validate(text).unwrap_err();
        let ConfigError::Ambiguous {
            field,
            v1_reading,
            toml_reading,
            as_written,
            field_name,
        } = err
        else {
            panic!("expected Ambiguous, got {err:?}");
        };
        assert_eq!(field, "remotes.default.path");
        assert_eq!(field_name, "path");
        assert_eq!(v1_reading, "datasets/run");
        assert_eq!(toml_reading, "datasets/run#1");
        assert_eq!(as_written, "\"datasets/run#1\"");
    }

    /// contract-spec §6.5 specifies this message's exact wording, not just its
    /// data -- "the message is doing the real work" and the `your objects are
    /// here` line is what lets a user resolve the ambiguity without guessing
    /// which reading is real.
    #[test]
    fn the_ambiguity_message_matches_the_contract_spec_template() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\npath = \"datasets/run#1\"\n";
        let err = parse_and_validate(text).unwrap_err();
        assert_eq!(
            err.to_string(),
            "error: .git-sfs/config.toml: remotes.default.path is ambiguous\n\
             \n  \
             as written:          \"datasets/run#1\"\n  \
             git-sfs 1.x read:     datasets/run      ← your objects are here\n  \
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
    fn an_entirely_unquoted_value_is_v1_only_syntax_accepted_via_the_legacy_reading() {
        // contract-spec §6.5 row 3: TOML fails (a bare word is not a valid
        // TOML value), v1 succeeds -- use v1's reading rather than erroring.
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
    fn rejects_a_config_v1_cannot_read_even_though_toml_can() {
        // A genuine TOML array: strict TOML fails to deserialize it into the
        // scalar `retry_max: Option<i64>` field, and v1's Atoi-based scanner
        // also rejects it as non-integer -- both fail, landing in the
        // both-failed case rather than silently trusting either.
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
        // `[remotes.""]` is syntactically legal TOML (a quoted empty string
        // key), unlike v1's bare `[remotes.]` -- this is exactly the case
        // `parse_raw`'s dedicated check exists for.
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

    /// A semantic rejection must propagate even when v1's naive scanner
    /// "succeeds" only because comment-truncation happens to land on a value
    /// that still looks valid. Found while fixing the `[remotes.""]` case
    /// above: without distinguishing `StrictTomlError::Toml` (a genuine TOML
    /// grammar failure, where deferring to v1 is correct) from a semantic
    /// failure like `UnsupportedAlgorithm`, this would have silently accepted
    /// v1's truncated `"sha256"` reading of a value that is actually
    /// `"sha256#not-sha256"` -- hiding a real misconfiguration.
    #[test]
    fn a_semantic_rejection_is_not_masked_by_v1_truncating_a_value_into_something_valid_looking() {
        let text = "version = 1\n[remotes.default]\nbackend = \"m\"\n[settings]\nalgorithm = \"sha256#not-sha256\"\n";
        assert!(matches!(
            parse_and_validate(text),
            Err(ConfigError::Invalid(_))
        ));
    }
}
