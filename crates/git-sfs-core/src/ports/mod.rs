//! The only side-effecting seams — rust-rewrite-plan §3.1.
//!
//! Everything in [`crate::domain`] is a pure value or a pure transform.
//! Everything here touches the outside world: the filesystem, a subprocess, a
//! clock. Commands (Phase 4) are built by composing these with `domain` and
//! `plan`; nothing above this layer is allowed to reach past it and touch I/O
//! directly.
//!
//! `Store` gets a trait because there are genuinely two implementations —
//! [`store::FsStore`] and a test fake — which is the bar rust-rewrite-plan
//! §3.3 sets for introducing one at all. `Lock` deliberately does not: one
//! real implementation, no second one contemplated. `Remote` and `Repo` are
//! not yet built.

pub mod cancellable_io;
pub mod lock;
pub mod store;

pub use cancellable_io::{Cancellable, is_canceled};
pub use lock::{Lock, LockError, LockName};
pub use store::{CacheEntry, FsStore, Store, StoreError};
