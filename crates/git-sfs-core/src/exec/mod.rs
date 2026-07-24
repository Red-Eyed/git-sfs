//! Command orchestration — rust-rewrite-plan §3.1's `exec` layer. Composes
//! `domain`, `plan`, and `ports` to do the actual work of a command.
//!
//! Still bound by this crate's whole invariant: **cannot print.** Functions
//! here return data for the binary to render (`AddOutcome`, an `AddError`,
//! …); none of them take a writer, a `quiet` flag, or a progress callback.
//! See each module's own doc for what it deliberately leaves out for this
//! reason and why — progress reporting in particular is Phase 5's job,
//! added once via an `Event` stream rather than threaded ad hoc into each
//! command as it's ported.

pub mod add;
pub mod import;
pub mod mv;
pub mod remotes;
pub mod status;

use camino::{Utf8Path, Utf8PathBuf};

/// `absolute`, expressed relative to `repo` -- v1's `rel()` (`walk.go:87`).
/// Shared by [`mv`] and [`import`], both of which report paths relative to
/// the repository root rather than as absolute filesystem paths.
pub(crate) fn repo_relative(repo: &Utf8Path, absolute: &Utf8Path) -> Utf8PathBuf {
    absolute.strip_prefix(repo).unwrap_or(absolute).to_owned()
}
