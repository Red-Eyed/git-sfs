//! Pure planning.
//!
//! No I/O, no clock: every function here takes data the `exec` layer already
//! obtained from a `ports::Store`/`ports::Remote`/`ports::Repo` call and
//! decides what should happen next. Commands become `scan -> plan -> execute
//! -> report`. The input shapes below are plain data, not port trait objects,
//! which keeps this layer testable with zero filesystem.

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
