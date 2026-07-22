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
