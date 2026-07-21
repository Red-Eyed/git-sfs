//! Pure domain values: illegal states unrepresentable, zero I/O.
//!
//! rust-rewrite-plan §3.1. Every type and function here is a value or a
//! deterministic transform over values already in hand — no filesystem, no
//! network, no clock. Where the real thing (a symlink, a config file) lives on
//! disk, the pattern is the same throughout: whoever reads it does the I/O
//! (a Phase 3 port), and hands the resulting bytes/text to a function here to
//! interpret. That split is what makes this module testable with zero
//! filesystem and is why every test in it runs in milliseconds.

pub mod config;
pub mod hash;
pub mod remote;
pub mod symlink;
pub mod version_floor;

pub use config::{Config, ConfigError, RemoteConfig, Settings};
pub use hash::{HashParseError, Sha256};
pub use remote::{DEFAULT_REMOTE_NAME, EmptyRemoteName, RemoteName, compose_remote_url};
pub use symlink::{
    InvalidSymlinkTarget, NoRelativePath, cache_link_file, git_link_target, validate_symlink_target,
};
pub use version_floor::{
    VersionCheckError, VersionParseError, VersionTriple, check_git_sfs_version,
    check_rclone_version,
};
