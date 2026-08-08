//! The user-visible version string.

/// The version `--version` prints.
///
/// Release builds inject the tag-shaped version string. Cargo's `version`
/// field cannot supply that form because Cargo metadata must be plain semver.
///
/// Development builds report `dev`.
pub const VERSION: &str = match option_env!("GIT_SFS_VERSION") {
    Some(version) => version,
    None => "dev",
};
