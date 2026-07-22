//! Pure planning — rust-rewrite-plan §3.1. No I/O, no clock: every function
//! here takes data the `exec` layer already obtained from a `ports::Store`/
//! `ports::Remote`/`ports::Repo` call and decides what should happen next.
//! Commands become `scan → plan → execute → report`; this module is the
//! `plan` step. It depends on nothing in `ports` — the input shapes below
//! are plain data shaped like what a port can cheaply produce, not port
//! trait objects, which is what keeps this layer testable with zero
//! filesystem.
//!
//! `plan_verify` is not here yet — deliberately deferred alongside the
//! `verify` command itself (Phase 4, see rust-rewrite-plan's Phase 2
//! checklist), since its planning shape is meaningfully richer than
//! push/pull's: eight issue kinds, a cheap-by-default vs. opt-in-expensive
//! remote check, and orphan detection needing the whole cache listing, not
//! just tracked links.

use camino::Utf8PathBuf;

use crate::domain::hash::Sha256;

pub mod disk_space;
pub mod pull;
pub mod push;

pub use disk_space::{InsufficientDiskSpace, check_disk_space, sum_needed_bytes};
pub use pull::{PullPlan, plan_pull};
pub use push::{PlanPushError, PushPlan, SkippedObject, plan_push};

/// One valid git-sfs symlink within the scope a command is operating on —
/// the shared input [`plan_push`]/[`plan_pull`] both plan around.
///
/// A `plan`-local type rather than reusing `ports::repo::ScannedEntry`
/// directly: this layer takes no dependency on `ports` at all (see the
/// module doc), and `ScannedEntry`'s `Invalid`/`Unrepresentable` variants are
/// exec's concern to filter out before planning, not plan's to represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedLink {
    /// Repo-relative path to the symlink.
    pub path: Utf8PathBuf,
    /// The hash its target names.
    pub hash: Sha256,
}
