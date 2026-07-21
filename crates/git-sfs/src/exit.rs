//! Failure class to process exit code.
//!
//! Exit codes live in the binary because core cannot exit. Core owns *which
//! class* a failure belongs to; this module owns *which number* that class
//! prints as. Splitting it that way is what keeps the taxonomy in one place
//! while leaving the process-level concern where it belongs.
//!
//! contract-spec 9 freezes exactly two things, and neither is really git-sfs's:
//! `0` versus non-zero, which is the Unix contract every CI script depends on,
//! and `130` for SIGINT, which is the shell's `128 + signal` convention.
//! Everything else is v2's to choose, and the choice is documented on
//! [`git_sfs_core::Error`].

use git_sfs_core::Error;

/// The command line was wrong.
pub const USAGE: u8 = 1;
/// The repository or cache setup was wrong.
pub const CONFIG: u8 = 2;
/// Bytes were wrong. Do not retry.
pub const INTEGRITY: u8 = 3;
/// Data was absent. Nothing is damaged.
pub const MISSING: u8 = 4;
/// git-sfs could not determine the answer. Retrying may help.
pub const UNAVAILABLE: u8 = 5;

/// Interrupted by the user.
///
/// Frozen at `128 + SIGINT` because every shell and CI runner already reads it
/// that way.
pub const CANCELED: u8 = 130;

/// `EX_SOFTWARE` from `sysexits.h`: the program itself is at fault.
///
/// Deliberately outside git-sfs's own range so that a stub command left behind
/// by the port is unmistakable at a glance. No released build may return it —
/// phase 4 removes the variant that produces it.
pub const UNIMPLEMENTED: u8 = 70;

/// The exit code for a failure.
#[must_use]
pub fn code_for(error: &Error) -> u8 {
    match error {
        Error::Usage(_) => USAGE,
        Error::Config(_) => CONFIG,
        Error::Integrity(_) => INTEGRITY,
        Error::Missing(_) => MISSING,
        Error::Unavailable(_) => UNAVAILABLE,
        Error::Canceled => CANCELED,
        Error::NotImplemented { .. } => UNIMPLEMENTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half of contract-spec 9 that is actually frozen: a failure must not
    /// look like success. Everything else in this table is v2's to change, but
    /// a zero here would make a broken repository pass CI.
    #[test]
    fn every_failure_exits_non_zero() {
        let failures = [
            Error::Usage(String::new()),
            Error::Config(String::new()),
            Error::Integrity(String::new()),
            Error::Missing(String::new()),
            Error::Unavailable(String::new()),
            Error::Canceled,
            Error::NotImplemented { command: "add" },
        ];

        for failure in &failures {
            assert_ne!(code_for(failure), 0, "{failure:?} exits 0");
        }
    }

    /// The other frozen half. `128 + SIGINT` belongs to the shell, not to us.
    #[test]
    fn cancellation_uses_the_shell_convention() {
        assert_eq!(code_for(&Error::Canceled), 130);
    }

    /// Two failures that must never share a code, because the whole point of
    /// the taxonomy is that one is worth retrying and the other is not.
    #[test]
    fn integrity_and_unavailable_are_distinguishable() {
        assert_ne!(
            code_for(&Error::Integrity(String::new())),
            code_for(&Error::Unavailable(String::new()))
        );
    }

    /// contract-spec 9.1: "missing is not corrupt". v1 stated it in prose and
    /// then mapped both to the same fall-through code.
    #[test]
    fn missing_is_not_corrupt() {
        assert_ne!(
            code_for(&Error::Missing(String::new())),
            code_for(&Error::Integrity(String::new()))
        );
    }
}
