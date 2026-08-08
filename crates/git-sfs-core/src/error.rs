//! The failure taxonomy.
//!
//! The grouping axis chosen here is **what the caller should do next**, because
//! the only consumers of exit codes are humans at a shell and CI scripts:
//!
//! | Variant | The caller should |
//! |---|---|
//! | [`Error::Usage`] | fix the command line |
//! | [`Error::Config`] | fix the repository or cache setup |
//! | [`Error::Integrity`] | investigate. Bytes are wrong; retrying cannot help |
//! | [`Error::Missing`] | run `pull` or `push`. Data is absent but nothing is broken |
//! | [`Error::Unavailable`] | retry. git-sfs could not determine the answer |
//! | [`Error::Canceled`] | nothing. The user asked for this |
//!
//! The last two rows carry the weight. Splitting "absent" from "could not
//! determine" prevents remote lookup failures from being reported as missing
//! user data.

use thiserror::Error;

/// A [`Result`](std::result::Result) whose error is git-sfs's taxonomy.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure git-sfs can report, grouped by what the caller should do.
///
/// The variants *are* the classification: the binary maps each one to an exit
/// code without inspecting anything further, so introducing a failure mode
/// means choosing its class here rather than editing a lookup table elsewhere.
///
/// Deliberately *not* `#[non_exhaustive]`. That attribute would force the
/// binary's exit-code mapping to carry a wildcard arm, and a new failure mode
/// would then inherit some existing code silently. Leaving the enum exhaustive
/// means adding a variant fails to compile until someone classifies it, which
/// is the whole reason the taxonomy lives in a type.
#[derive(Debug, Error)]
pub enum Error {
    /// The invocation is wrong: unknown flag, missing argument, bad value.
    ///
    /// Deterministic and entirely the caller's to fix. Re-running unchanged
    /// fails identically.
    #[error("{0}")]
    Usage(String),

    /// The repository or cache is not in a state the command can work from:
    /// no `.git-sfs/config.toml`, no cache binding, an unparseable config, an
    /// unknown remote name.
    ///
    /// Distinct from [`Usage`](Error::Usage) because the command line was fine
    /// and the fix is elsewhere.
    #[error("{0}")]
    Config(String),

    /// Bytes are wrong. A cache object whose content does not match its name, a
    /// remote object that failed verification, a symlink that does not point
    /// into the cache, a cache file left writable when it should be read-only.
    ///
    /// **Never retry.** Retrying an integrity failure produces the same result
    /// more slowly, and treating it as transient is how a corrupt object gets
    /// replicated. This is the class `verify` exists to detect and the reason
    /// it is usable as a CI gate.
    #[error("{0}")]
    Integrity(String),

    /// Data that should exist does not: a cache object the working tree refers
    /// to, or a remote object a `pull` needs.
    ///
    /// Nothing is damaged. The remedy is another git-sfs command, which is why
    /// this is not [`Integrity`](Error::Integrity).
    #[error("{0}")]
    Missing(String),

    /// git-sfs could not determine the answer: the remote refused the request,
    /// the credentials expired, rclone failed to run, a read errored.
    ///
    /// The defining property is *ignorance*, not absence. A command that cannot
    /// reach a remote must fail this way rather than reporting the remote as
    /// empty.
    #[error("{0}")]
    Unavailable(String),

    /// The user interrupted the run with SIGINT or SIGTERM.
    ///
    /// Cancellation outranks every other classification, so an interrupted run
    /// reports this even if the aborted operation produced some other error on
    /// its way out. The binary enforces that precedence in one place rather
    /// than per command.
    #[error("canceled")]
    Canceled,
}
