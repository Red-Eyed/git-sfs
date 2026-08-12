//! `min_git_sfs_version` / `min_rclone_version` comparison.
//!
//! Configured floor values are intentionally not parsed as semver. They accept
//! an optional leading `v` and leading zeros, and reject prerelease/build
//! metadata. The running git-sfs release is parsed as semver so a prerelease can
//! be compared correctly without widening the committed config grammar.
//!
//! `VersionTriple` remains separate from semver because widening the accepted
//! floor syntax would change which committed configs load.

use semver::Version;
use thiserror::Error;

/// A version compared as three lexicographically ordered integers.
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
    /// A running git-sfs release tag was not valid semantic versioning.
    #[error("invalid release version {input:?}: {reason}")]
    InvalidRelease {
        /// The rejected release string.
        input: String,
        /// The semantic-version parser's diagnostic.
        reason: String,
    },
}

impl VersionTriple {
    /// Parses `[v]major.minor.patch`.
    ///
    /// - An optional leading `v` is stripped.
    /// - Exactly three `.`-separated components are required. A fourth `.`
    ///   stays inside the patch component and then fails to parse as an
    ///   integer.
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
///, which is what keeps an unreleased build usable against
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
    let got = parse_release_version(current).map_err(VersionCheckError::Current)?;
    let min = VersionTriple::parse(minimum).map_err(VersionCheckError::Minimum)?;
    let min = Version::new(
        u64::from(min.0[0]),
        u64::from(min.0[1]),
        u64::from(min.0[2]),
    );
    if got >= min {
        return Ok(());
    }
    Err(VersionCheckError::BelowMinimum {
        current: current.to_owned(),
        minimum: minimum.to_owned(),
    })
}

/// Checks a detected rclone version against `min_rclone_version`. Unlike
/// [`check_git_sfs_version`], there is no `"dev"` bypass.
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

fn parse_release_version(input: &str) -> Result<Version, VersionParseError> {
    Version::parse(input.strip_prefix('v').unwrap_or(input)).map_err(|err| {
        VersionParseError::InvalidRelease {
            input: input.to_owned(),
            reason: err.to_string(),
        }
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

    /// Release versions carry a leading `v`; floor checks must accept that
    /// exact string so a repo can require the release currently running it.
    #[test]
    fn a_release_binaries_own_v_prefixed_version_satisfies_its_own_floor() {
        assert!(check_git_sfs_version("v9.0.0", "1.6.0").is_ok());
    }

    #[test]
    fn a_prerelease_satisfies_an_older_floor() {
        assert!(check_git_sfs_version("v2.0.0-rc.1", "1.6.0").is_ok());
    }

    #[test]
    fn a_prerelease_does_not_satisfy_its_final_release_floor() {
        let err = check_git_sfs_version("v2.0.0-rc.1", "2.0.0").unwrap_err();
        assert!(matches!(err, VersionCheckError::BelowMinimum { .. }));
    }

    #[test]
    fn malformed_running_release_is_rejected() {
        let err = check_git_sfs_version("v2.0.0-not semver", "1.6.0").unwrap_err();
        assert!(matches!(err, VersionCheckError::Current(_)));
    }

    #[test]
    fn below_minimum_is_reported_with_both_versions() {
        let err = check_git_sfs_version("1.5.0", "1.6.0").unwrap_err();
        assert!(matches!(err, VersionCheckError::BelowMinimum { .. }));
    }
}
