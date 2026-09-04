//! The side-effecting seams.
//!
//! Everything in [`crate::domain`] is a pure value or a pure transform.
//! Everything here touches the outside world: the filesystem, a subprocess, a
//! clock. Commands are built by composing these with `domain` and `plan`;
//! nothing above this layer reaches past it and touches I/O directly.
//!
//! `Store`, `Remote`, and `Repo` get traits because there are genuinely two
//! implementations each: a real one plus a test fake. `Lock` deliberately
//! does not; there is one lock protocol and every command uses it directly.
//! `local_state` is not a trait either, because repository discovery and cache
//! binding are concrete filesystem operations.

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
pub use store::{
    CacheEntry, DownloadVerification, FakeStore, FsStore, PullStore, Store, StoreError,
    purge_stale_tmp_files,
};
