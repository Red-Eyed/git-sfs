//! Pure domain values: illegal states unrepresentable, zero I/O.
//!
//! Every type and function here is a value or a deterministic transform over
//! values already in hand: no filesystem, no network, no clock. Where the real
//! thing lives on disk, a port reads the bytes or text and passes them here for
//! interpretation.

pub mod cache_layout;
pub mod config;
pub mod hash;
pub mod remote;
pub mod symlink;
pub mod version_floor;

pub use cache_layout::{locks_dir, object_path, tmp_dir, trash_dir};
pub use config::{Config, ConfigError, RemoteConfig, Settings};
pub use hash::{HashParseError, Sha256};
pub use remote::{
    DEFAULT_REMOTE_NAME, EmptyRemoteName, RemoteName, compose_remote_url, object_url,
};
pub use symlink::{
    InvalidSymlinkTarget, NoRelativePath, cache_link_file, git_link_target, validate_symlink_target,
};
pub use version_floor::{
    VersionCheckError, VersionParseError, VersionTriple, check_git_sfs_version,
    check_rclone_version,
};
