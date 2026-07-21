//! git-sfs core: everything that decides *what should happen*.
//!
//! This crate cannot print and cannot exit. It does not depend on the binary
//! crate, on `clap`, or on any terminal library, so the functional-core /
//! imperative-shell boundary is enforced by the dependency graph rather than by
//! anyone remembering it (rust-rewrite-plan 3).
//!
//! In particular, no function here takes a writer, a `quiet` flag, or a
//! progress callback. Callers that want to observe an operation consume the
//! event stream the `exec` layer emits; they do not pass observation down into
//! the logic. That is the Open/Closed violation the rewrite exists to delete
//! (rust-rewrite-plan 3.2).

#![warn(missing_docs)]

pub mod cancel;
pub mod domain;
pub mod error;
pub mod ports;

pub use cancel::Cancel;
pub use error::{Error, Result};
