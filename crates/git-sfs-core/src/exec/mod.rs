//! Command orchestration.
//!
//! Still bound by this crate's whole invariant: **cannot print.** Functions
//! here return data for the binary to render (`AddOutcome`, an `AddError`,
//! …); none of them take a writer, a `quiet` flag, or a progress callback.

pub mod add;
pub mod doctor;
pub mod import;
pub mod init;
pub mod mv;
pub mod pull;
pub mod push;
pub mod remotes;
pub mod setup;
pub mod status;
pub mod verify;

use camino::{Utf8Path, Utf8PathBuf};

/// `absolute`, expressed relative to `repo`.
/// Shared by [`mv`] and [`import`], both of which report paths relative to
/// the repository root rather than as absolute filesystem paths.
pub(crate) fn repo_relative(repo: &Utf8Path, absolute: &Utf8Path) -> Utf8PathBuf {
    absolute.strip_prefix(repo).unwrap_or(absolute).to_owned()
}
