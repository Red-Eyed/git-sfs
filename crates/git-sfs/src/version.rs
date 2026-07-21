//! The user-visible version string.

/// The version `--version` prints.
///
/// contract-spec 11 requires this to stay parseable as the release tag, in
/// `v1.21.0` form, because the workflow suite asserts on it and installed v1
/// binaries resolve release archives by it. That form is not valid semver
/// (contract-spec 6.6), so it cannot be derived from Cargo's `version` field
/// and is injected by the release build instead.
///
/// Development builds report `dev`, matching v1.
pub const VERSION: &str = match option_env!("GIT_SFS_VERSION") {
    Some(version) => version,
    None => "dev",
};
