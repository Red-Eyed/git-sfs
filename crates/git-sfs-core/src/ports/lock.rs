//! Inter-process mutual exclusion — contract-spec §8.
//!
//! **Not a trait.** There is one real implementation (`mkdir`-based, frozen
//! name and mechanism) and no second one is contemplated, so per
//! rust-rewrite-plan §3.3 this does not clear the bar for an abstraction —
//! unlike [`super::store::Store`], which earns its trait from genuinely
//! having two.
//!
//! **This is an inter-version contract, not an implementation detail.**
//! During migration a user will plausibly run a v1 binary in one shell and
//! v2 in another against the same cache. The five lock names in [`LockName`]
//! and the `mkdir`/`owner`-file mechanism below are frozen exactly as
//! `lock.go` defines them; changing either means both binaries can hold
//! "the lock" on the same cache at once.
//!
//! What v2 does differently is policy, not mechanism (contract-spec §8.1):
//! v1 spins on `os.Mkdir` forever, never checking whether the recorded owner
//! PID is still alive, so a crashed/SIGKILLed/OOM-killed holder leaves a
//! lock that blocks every future `add`/`import`/`push`/`pull` on that cache
//! permanently, recoverable only by `rm -rf` inside the content-addressed
//! store. [`Lock::acquire`] checks liveness and auto-breaks a *definitely*
//! dead owner; [`Lock::force_break`] is the documented, named escape hatch
//! for the cases liveness checking cannot resolve on its own.

use std::io;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::error::Error;

/// Poll interval while waiting on a contended lock — contract-spec §8,
/// frozen at 100ms alongside the rest of the mechanism.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One of the five frozen lock names (contract-spec §8.2). A closed enum
/// rather than a bare `&str`: the whole point of §8.2 is that these five
/// names must never drift, and an exhaustive `match` on this type makes a
/// typo'd or silently-renamed sixth variant a compile error instead of a
/// cross-version exclusion failure discovered in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockName {
    /// `locks/add.lock` — `add.go:40`.
    Add,
    /// `locks/import.lock` — `import.go:59`.
    Import,
    /// `locks/setup.lock` — `init.go:90`.
    Setup,
    /// `locks/pull.lock` — `pull.go:33`.
    Pull,
    /// `locks/push.lock` — `push.go:42`.
    Push,
}

impl LockName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Import => "import",
            Self::Setup => "setup",
            Self::Pull => "pull",
            Self::Push => "push",
        }
    }
}

/// Why a [`Lock`] operation failed.
#[derive(Debug, Error)]
pub enum LockError {
    /// An I/O operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The caller asked to stop while waiting for a contended lock.
    #[error("canceled")]
    Canceled,
}

impl From<LockError> for Error {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Io { .. } => Error::Unavailable(err.to_string()),
            LockError::Canceled => Error::Canceled,
        }
    }
}

/// A held lock directory: `<cache_root>/locks/<name>.lock/`. Dropping it
/// releases the lock.
#[derive(Debug)]
pub struct Lock {
    path: Utf8PathBuf,
}

impl Lock {
    /// Blocks until `name`'s lock is held, or `cancel` fires.
    ///
    /// Polls every 100ms (contract-spec §8, frozen). On contention, reads
    /// the contending lock's `owner` file and checks that PID for liveness
    /// (contract-spec §8.1): a definitely-dead owner (`kill(pid, 0)` →
    /// `ESRCH`) is broken and acquisition retried immediately. A missing,
    /// empty, or malformed owner file, or one naming a live PID, is treated
    /// as "possibly alive" and the poll continues — this is deliberately
    /// conservative, since v1 itself sometimes fails to write the owner file
    /// (`lock.go:33` ignores that error) and a live v1 holder must never be
    /// broken.
    ///
    /// **Known limitation, not fixed here:** a PID can be reused by an
    /// unrelated process between the original holder crashing and this
    /// check running, in which case a dead owner reads as alive. This errs
    /// in the conservative direction — it can only make a stale lock wait
    /// longer, never break a live one — and [`Lock::force_break`] is the
    /// deliberate override for that case.
    ///
    /// # Errors
    ///
    /// Returns [`LockError::Canceled`] if `cancel` fires before the lock is
    /// acquired, and [`LockError::Io`] for any other failure.
    pub fn acquire(
        locks_dir: &Utf8Path,
        name: LockName,
        cancel: &Cancel,
    ) -> Result<Self, LockError> {
        std::fs::create_dir_all(locks_dir).map_err(|source| LockError::Io {
            path: locks_dir.to_owned(),
            source,
        })?;
        let path = locks_dir.join(format!("{}.lock", name.as_str()));

        loop {
            if cancel.is_canceled() {
                return Err(LockError::Canceled);
            }

            match std::fs::create_dir(&path) {
                Ok(()) => {
                    if let Err(source) = write_owner(&path) {
                        // Cannot record who holds this lock -- no future
                        // waiter could ever tell this holder apart from a
                        // dead one. Fail loudly rather than publish a lock
                        // nothing can later auto-break.
                        release(&path);
                        return Err(LockError::Io { path, source });
                    }
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    if read_owner_pid(&path).is_some_and(|pid| !is_alive(pid)) {
                        release(&path);
                        continue; // retry the mkdir immediately, no sleep
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(source) => return Err(LockError::Io { path, source }),
            }
        }
    }

    /// Unconditionally removes `name`'s lock, regardless of whether its
    /// owner is alive. The documented break-glass path contract-spec §8.1
    /// requires so that recovery from a lock liveness-checking cannot
    /// resolve on its own — a reused PID, a network-mounted cache shared
    /// with a host this process cannot see — is never "delete things inside
    /// the data store by hand."
    ///
    /// A no-op, not an error, if the lock is not currently held.
    ///
    /// # Errors
    ///
    /// Returns [`LockError::Io`] if the lock directory exists but could not
    /// be removed.
    pub fn force_break(locks_dir: &Utf8Path, name: LockName) -> Result<(), LockError> {
        let path = locks_dir.join(format!("{}.lock", name.as_str()));
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(LockError::Io { path, source }),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        release(&self.path);
    }
}

/// Best-effort lock removal, shared by [`Lock::acquire`]'s dead-owner
/// break, its own write-owner failure cleanup, and [`Drop for
/// Lock`](Lock)'s release. A failure here cannot be surfaced from `Drop`,
/// and in the dead-owner case a failure just means the next waiter retries
/// the same check rather than compounding the error.
fn release(path: &Utf8Path) {
    #[allow(
        clippy::let_underscore_must_use,
        reason = "best-effort release; Drop has no way to propagate a failure and the caller-facing paths that call this return their own error before reaching it"
    )]
    let _ = std::fs::remove_dir_all(path);
}

/// Writes the `owner` file recording this process's PID — advisory content
/// (contract-spec §8: wording is not frozen), but presence and location are,
/// since a waiting process reads it for the liveness check above.
fn write_owner(lock_path: &Utf8Path) -> io::Result<()> {
    std::fs::write(
        lock_path.join("owner"),
        format!("pid: {}\n", std::process::id()),
    )
}

/// Reads the owning PID from `lock_path`'s `owner` file, or `None` if it is
/// missing, empty, or malformed. Deliberately infallible rather than
/// panicking: v1's own reader (`lock.go:62`) slices `data[:len(data)-1]`
/// with no length check against a file whose creation (`lock.go:33`)
/// ignores its own write error, so a zero-byte owner file is reachable in
/// practice and v2 must tolerate it rather than crash reading a v1-held
/// lock. A non-positive parsed value is also treated as malformed -- `kill`
/// gives `0` and negative PIDs process-group/broadcast semantics that do not
/// mean what a single-process liveness check needs here.
fn read_owner_pid(lock_path: &Utf8Path) -> Option<i32> {
    let data = std::fs::read_to_string(lock_path.join("owner")).ok()?;
    let pid: i32 = data.trim().strip_prefix("pid:")?.trim().parse().ok()?;
    (pid > 0).then_some(pid)
}

/// Whether `pid` names a running process, via `kill(pid, 0)` -- the
/// standard POSIX existence check: signal `0` delivers nothing but the
/// kernel still performs the permission/existence check before not sending
/// it. Only a definite `ESRCH` ("no such process") reports dead; every other
/// outcome, including `EPERM` (exists, owned by someone else) and any
/// unexpected errno, reports alive. This asymmetry is deliberate: acquiring
/// is safe to delay, but wrongly breaking a live process's lock is not, so
/// ambiguity must resolve toward "alive."
fn is_alive(pid: i32) -> bool {
    !matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn locks_dir(cache: &tempfile::TempDir) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(cache.path().join("locks")).unwrap()
    }

    /// A PID guaranteed not to belong to any running process: spawn a child
    /// and wait for it to exit.
    fn a_definitely_dead_pid() -> i32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawning `true` for a disposable pid");
        let pid = child.id() as i32;
        child.wait().expect("reaping the disposable child");
        pid
    }

    #[test]
    fn acquiring_an_uncontended_lock_succeeds_and_records_the_owner() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        let cancel = Cancel::new();

        let lock = Lock::acquire(&dir, LockName::Add, &cancel).unwrap();
        let lock_path = dir.join("add.lock");
        assert!(lock_path.is_dir());
        let owner = std::fs::read_to_string(lock_path.join("owner")).unwrap();
        assert_eq!(owner, format!("pid: {}\n", std::process::id()));

        drop(lock);
        assert!(!lock_path.exists(), "dropping the lock must release it");
    }

    #[test]
    fn each_of_the_five_frozen_names_maps_to_its_own_directory() {
        // Direct regression guard on contract-spec §8.2's frozen table --
        // this must never be "cleaned up" to a shared constant.
        let cases = [
            (LockName::Add, "add"),
            (LockName::Import, "import"),
            (LockName::Setup, "setup"),
            (LockName::Pull, "pull"),
            (LockName::Push, "push"),
        ];
        for (name, expected) in cases {
            assert_eq!(name.as_str(), expected);
        }
    }

    #[test]
    fn acquiring_blocks_while_a_live_owner_holds_the_lock_and_unblocks_on_release() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("push.lock");
        std::fs::create_dir(&lock_path).unwrap();
        // This test process is itself alive, so recording our own pid as
        // the owner simulates a live holder without needing a second process.
        write_owner(&lock_path).unwrap();

        let (tx, rx) = mpsc::channel();
        let waiter_dir = dir.clone();
        let handle = std::thread::spawn(move || {
            let cancel = Cancel::new();
            let result = Lock::acquire(&waiter_dir, LockName::Push, &cancel);
            tx.send(()).unwrap();
            result
        });

        // Must still be blocked after a few poll intervals.
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "acquire must not succeed while the live owner still holds the lock"
        );

        std::fs::remove_dir_all(&lock_path).unwrap();
        let result = handle.join().unwrap();
        assert!(
            result.is_ok(),
            "acquire must succeed once the lock is released"
        );
    }

    #[test]
    fn contention_poll_interval_is_frozen_at_100ms() {
        assert_eq!(POLL_INTERVAL, Duration::from_millis(100));
    }

    #[test]
    fn acquiring_auto_breaks_a_lock_whose_owner_is_provably_dead() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("pull.lock");
        std::fs::create_dir(&lock_path).unwrap();
        std::fs::write(
            lock_path.join("owner"),
            format!("pid: {}\n", a_definitely_dead_pid()),
        )
        .unwrap();

        let cancel = Cancel::new();
        // Bounded by the test harness's own timeout rather than looping
        // forever if this regresses back to v1's indefinite wait.
        let lock = Lock::acquire(&dir, LockName::Pull, &cancel)
            .expect("a dead owner's lock must be broken and reacquired promptly");
        drop(lock);
    }

    #[test]
    fn acquiring_does_not_auto_break_a_lock_with_no_readable_owner() {
        // Conservative-by-construction: an owner file this process cannot
        // interpret must never be treated as proof of death, since a live
        // v1 holder can legitimately have one (lock.go:33 swallows its own
        // write error).
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("import.lock");
        std::fs::create_dir(&lock_path).unwrap();
        // No owner file at all.

        let (tx, rx) = mpsc::channel();
        let waiter_dir = dir.clone();
        std::thread::spawn(move || {
            let cancel = Cancel::new();
            // The test only cares that `acquire` returned, not which way --
            // that's asserted separately below via the channel timing.
            let acquired = Lock::acquire(&waiter_dir, LockName::Import, &cancel);
            tx.send(()).unwrap();
            acquired
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "an unreadable owner must not be auto-broken"
        );

        std::fs::remove_dir_all(&lock_path).unwrap();
        rx.recv_timeout(Duration::from_secs(2))
            .expect("acquire proceeds once the lock is manually cleared");
    }

    #[test]
    fn acquiring_stops_promptly_on_cancellation() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("setup.lock");
        std::fs::create_dir(&lock_path).unwrap();
        write_owner(&lock_path).unwrap(); // live owner (this process)

        let cancel = Cancel::new();
        cancel.cancel();
        let err = Lock::acquire(&dir, LockName::Setup, &cancel).unwrap_err();
        assert!(matches!(err, LockError::Canceled));
    }

    #[test]
    fn force_break_removes_a_lock_even_with_a_live_owner() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        let cancel = Cancel::new();
        let lock = Lock::acquire(&dir, LockName::Add, &cancel).unwrap();
        let lock_path = dir.join("add.lock");
        assert!(lock_path.is_dir());

        Lock::force_break(&dir, LockName::Add).unwrap();
        assert!(!lock_path.exists());

        // The original `Lock` handle's Drop must not error even though the
        // directory it thinks it owns is already gone.
        drop(lock);
    }

    #[test]
    fn force_break_on_an_unheld_lock_is_a_no_op() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        Lock::force_break(&dir, LockName::Import).unwrap();
    }

    #[test]
    fn a_missing_empty_or_malformed_owner_file_never_panics() {
        let cache = tempfile::tempdir().unwrap();
        let dir = locks_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("push.lock");
        std::fs::create_dir(&lock_path).unwrap();

        assert_eq!(read_owner_pid(&lock_path), None, "missing owner file");

        std::fs::write(lock_path.join("owner"), "").unwrap();
        assert_eq!(read_owner_pid(&lock_path), None, "zero-byte owner file");

        std::fs::write(lock_path.join("owner"), "not a pid at all\n").unwrap();
        assert_eq!(read_owner_pid(&lock_path), None, "malformed owner file");

        std::fs::write(lock_path.join("owner"), "pid: -1\n").unwrap();
        assert_eq!(read_owner_pid(&lock_path), None, "non-positive pid");

        std::fs::write(
            lock_path.join("owner"),
            format!("pid: {}\n", std::process::id()),
        )
        .unwrap();
        assert_eq!(read_owner_pid(&lock_path), Some(std::process::id() as i32));
    }

    #[test]
    fn liveness_check_treats_the_current_process_as_alive_and_a_reaped_child_as_dead() {
        assert!(is_alive(std::process::id() as i32));
        assert!(!is_alive(a_definitely_dead_pid()));
    }
}
