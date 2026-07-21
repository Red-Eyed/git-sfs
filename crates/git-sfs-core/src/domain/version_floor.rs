//! `min_git_sfs_version` / `min_rclone_version` comparison.
//!
//! contract-spec §6.6: this is **not** semver, and every difference is
//! load-bearing. `semver::Version::parse` — the crate rust-rewrite-plan §4.1
//! otherwise adopts — inverts all three rules below, so it cannot be used
//! here: it rejects a leading `v`, rejects leading zeros, and accepts
//! prerelease. Using it unmodified would not be a refactor, it would change
//! which committed configs load, and it would reject git-sfs's *own* version
//! string (§11 pins `--version` to the `v1.21.0` tag form), breaking every
//! repo that sets `min_git_sfs_version` in one release.
//!
//! `semver` remains the right crate wherever this project wants genuine
//! semver compliance; this module exists because that is not one of those
//! places.

use thiserror::Error;

/// A version compared as three lexicographically-ordered integers — nothing
/// more. Derived `Ord` on `[u32; 3]` is exactly v1's comparison loop
/// (`config.go:49-57`): compare component by component, first difference
/// decides, equal falls through to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionTriple([u32; 3]);

/// Why a version string failed to parse.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VersionParseError {
    /// Not exactly three `.`-separated components.
    #[error("invalid version {input:?}: expected major.minor.patch")]
    WrongComponentCount {
        /// The rejected input, for the error message.
        input: String,
    },
    /// A component was not a bare non-negative integer — this is also how
    /// prerelease/build metadata is rejected: `"0-beta".parse::<u32>()` fails
    /// the same way a genuinely malformed component would.
    #[error("invalid version {input:?}: component {component:?} is not an integer")]
    ComponentNotAnInteger {
        /// The rejected input, for the error message.
        input: String,
        /// The specific component that failed to parse.
        component: String,
    },
}

impl VersionTriple {
    /// Parses `[v]major.minor.patch`.
    ///
    /// - An optional leading `v` is stripped (`config.go:19`).
    /// - Exactly three `.`-separated components are required, matching Go's
    ///   `SplitN(s, ".", 3)`: a fourth `.` does not split further, it becomes
    ///   part of the third component and then fails to parse as an integer.
    /// - Leading zeros are accepted (`"1.07.0"` reads as `1.7.0`).
    /// - Prerelease and build metadata are rejected: `"1.67.0-beta"` is an
    ///   error, because `"0-beta"` is not a bare integer.
    ///
    /// # Errors
    ///
    /// Returns [`VersionParseError`] if `s` does not have this shape.
    pub fn parse(s: &str) -> Result<Self, VersionParseError> {
        let unprefixed = s.strip_prefix('v').unwrap_or(s);
        let parts: Vec<&str> = unprefixed.splitn(3, '.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(VersionParseError::WrongComponentCount {
                input: s.to_owned(),
            });
        };

        let component = |c: &str| {
            c.parse::<u32>()
                .map_err(|_| VersionParseError::ComponentNotAnInteger {
                    input: s.to_owned(),
                    component: c.to_owned(),
                })
        };
        Ok(Self([
            component(major)?,
            component(minor)?,
            component(patch)?,
        ]))
    }
}

/// Why a version check failed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VersionCheckError {
    /// `current` did not parse as a [`VersionTriple`].
    #[error("parse current version: {0}")]
    Current(#[source] VersionParseError),
    /// `minimum` (the configured floor) did not parse as a [`VersionTriple`].
    #[error("parse minimum version: {0}")]
    Minimum(#[source] VersionParseError),
    /// Both parsed; `current` is below `minimum`.
    #[error("{current} is below required {minimum}")]
    BelowMinimum {
        /// The version that was checked.
        current: String,
        /// The configured floor it did not meet.
        minimum: String,
    },
}

/// Checks git-sfs's own version against `min_git_sfs_version`.
///
/// `current == "dev"` always passes — development builds are never blocked
/// (`config.go:38-40`), which is what keeps an unreleased build usable against
/// a repo that sets a floor.
///
/// # Errors
///
/// Returns [`VersionCheckError`] if either string fails to parse, or if
/// `current` is below `minimum`.
pub fn check_git_sfs_version(current: &str, minimum: &str) -> Result<(), VersionCheckError> {
    if current == "dev" {
        return Ok(());
    }
    check(current, minimum)
}

/// Checks a detected rclone version against `min_rclone_version`. Unlike
/// [`check_git_sfs_version`], there is no `"dev"` bypass — v1 has none either.
///
/// # Errors
///
/// Returns [`VersionCheckError`] if either string fails to parse, or if
/// `detected` is below `minimum`.
pub fn check_rclone_version(detected: &str, minimum: &str) -> Result<(), VersionCheckError> {
    check(detected, minimum)
}

fn check(current: &str, minimum: &str) -> Result<(), VersionCheckError> {
    let got = VersionTriple::parse(current).map_err(VersionCheckError::Current)?;
    let min = VersionTriple::parse(minimum).map_err(VersionCheckError::Minimum)?;
    if got >= min {
        return Ok(());
    }
    Err(VersionCheckError::BelowMinimum {
        current: current.to_owned(),
        minimum: minimum.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_v_is_stripped() {
        assert_eq!(
            VersionTriple::parse("v1.67.0").unwrap(),
            VersionTriple::parse("1.67.0").unwrap()
        );
    }

    #[test]
    fn leading_zeros_are_accepted() {
        assert_eq!(
            VersionTriple::parse("1.07.0").unwrap(),
            VersionTriple::parse("1.7.0").unwrap()
        );
    }

    #[test]
    fn exactly_three_components_are_required() {
        assert!(VersionTriple::parse("1.60").is_err());
    }

    #[test]
    fn a_fourth_component_does_not_split_further_and_fails_to_parse() {
        // SplitN(s, ".", 3) semantics: "1.2.3.4" -> ["1", "2", "3.4"], and
        // "3.4" is not a bare integer.
        assert!(VersionTriple::parse("1.2.3.4").is_err());
    }

    #[test]
    fn prerelease_metadata_is_rejected() {
        assert!(VersionTriple::parse("1.67.0-beta").is_err());
    }

    #[test]
    fn comparison_is_lexicographic_with_early_exit() {
        let earlier = VersionTriple::parse("1.9.9").unwrap();
        let later = VersionTriple::parse("1.10.0").unwrap();
        // A naive string or per-digit-without-parsing comparison would rank
        // "1.9.9" above "1.10.0"; parsing each component as an integer first
        // is what makes 10 > 9 rather than '1' < '9'.
        assert!(later > earlier);
    }

    #[test]
    fn dev_bypasses_the_git_sfs_version_floor() {
        assert!(check_git_sfs_version("dev", "999.0.0").is_ok());
    }

    #[test]
    fn dev_does_not_bypass_the_rclone_floor() {
        assert!(check_rclone_version("dev", "1.0.0").is_err());
    }

    /// The sharp edge rust-rewrite-plan §6.6 names directly: `bare semver`
    /// rejects a leading `v`, and git-sfs's own `--version` output is pinned
    /// to the `v1.21.0` tag form (contract-spec §11). If this parsed with
    /// `semver::Version::parse` instead, a repo with `min_git_sfs_version`
    /// set would fail to parse the running binary's *own* version and error
    /// on every invocation.
    #[test]
    fn a_release_binaries_own_v_prefixed_version_satisfies_its_own_floor() {
        assert!(check_git_sfs_version("v1.21.0", "1.6.0").is_ok());
    }

    #[test]
    fn below_minimum_is_reported_with_both_versions() {
        let err = check_git_sfs_version("1.5.0", "1.6.0").unwrap_err();
        assert!(matches!(err, VersionCheckError::BelowMinimum { .. }));
    }
}
