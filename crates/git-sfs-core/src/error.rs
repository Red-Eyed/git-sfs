//! The failure taxonomy.
//!
//! contract-spec 12 unfreezes v1's exit-code taxonomy, so v2 designs its own.
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
//! The last two rows carry the weight. v1 collapsed "absent" and "could not
//! determine" into one `(false, nil)` return and then reported an unreachable
//! remote as a remote holding none of the user's data (contract-spec 13.3).
//! Splitting them at the top of the taxonomy makes that conflation something a
//! developer has to type on purpose, rather than something a signature invites.

use thiserror::Error;

/// A [`Result`](std::result::Result) whose error is git-sfs's taxonomy.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure git-sfs can report, grouped by what the caller should do.
///
/// The variants *are* the classification: the binary maps each one to an exit
/// code without inspecting anything further, so introducing a failure mode
/// means choosing its class here rather than editing a lookup table elsewhere.
///
/// The `String` payloads are scaffolding. Phase 2 replaces them with typed
/// domain errors carried as `#[source]`, at which point the message stops being
/// the only machine-readable thing about a failure. The classification boundary
/// is what phase 1 needs to get right, and that does not depend on the payload.
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
    /// it is usable as a CI gate (contract-spec 9.1).
    #[error("{0}")]
    Integrity(String),

    /// Data that should exist does not: a cache object the working tree refers
    /// to, or a remote object a `pull` needs.
    ///
    /// Nothing is damaged. The remedy is another git-sfs command, which is why
    /// this is not [`Integrity`](Error::Integrity) — contract-spec 9.1 is
    /// explicit that missing is not corrupt.
    #[error("{0}")]
    Missing(String),

    /// git-sfs could not determine the answer: the remote refused the request,
    /// the credentials expired, rclone failed to run, a read errored.
    ///
    /// The defining property is *ignorance*, not absence. A command that cannot
    /// reach a remote must fail this way rather than reporting the remote as
    /// empty, which is the single defect contract-spec 13.3 catalogues five
    /// instances of.
    #[error("{0}")]
    Unavailable(String),

    /// The user interrupted the run with SIGINT or SIGTERM.
    ///
    /// The message is frozen: contract-spec 9 requires `git-sfs: canceled` on
    /// stderr. Cancellation also outranks every other classification, so a run
    /// that was interrupted reports this even if the aborted operation
    /// produced some other error on its way out. The binary enforces that
    /// precedence in one place rather than per command.
    #[error("canceled")]
    Canceled,

    /// The command parses but this build cannot run it yet.
    ///
    /// Scaffolding for the port: it exists so that an unported command fails
    /// loudly instead of exiting 0 and being read as success by the
    /// differential harness. Phase 4 removes the last of these along with the
    /// variant itself.
    #[error("{command} is not implemented in this build")]
    NotImplemented {
        /// The command as it was written on the command line.
        command: &'static str,
    },
}
