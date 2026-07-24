//! The only side-effecting seams — rust-rewrite-plan §3.1.
//!
//! Everything in [`crate::domain`] is a pure value or a pure transform.
//! Everything here touches the outside world: the filesystem, a subprocess, a
//! clock. Commands (Phase 4) are built by composing these with `domain` and
//! `plan`; nothing above this layer is allowed to reach past it and touch I/O
//! directly.
//!
//! `Store`, `Remote`, and `Repo` get traits because there are genuinely two
//! implementations each — a real one plus a test fake — which is the bar
//! rust-rewrite-plan §3.3 sets for introducing one at all. `Lock`
//! deliberately does not: one real implementation, no second one
//! contemplated. `local_state` isn't a trait either, for the same reason as
//! `Lock` — there is exactly one way to walk up for `.git` or read a
//! symlink, no second implementation on the horizon.

pub mod cancellable_io;
pub mod hashing;
pub mod local_state;
pub mod lock;
pub mod remote;
pub mod repo;
pub mod store;

pub use cancellable_io::{Cancellable, is_canceled};
pub use hashing::{hash_file, hash_reader};
pub use local_state::{
    LocalStateError, bind_cache, choose_cache_root, discover_repo, init_cache_dirs,
    init_git_sfs_dir, resolve_cache_root,
};
pub use lock::{Lock, LockError, LockName};
pub use remote::{FakeRemote, RcloneRemote, Remote, RemoteError, detect_rclone_version};
pub use repo::{FakeRepo, FoundEntry, FsRepo, InvalidReason, Repo, RepoError, ScannedEntry};
pub use store::{CacheEntry, FakeStore, FsStore, Store, StoreError};
