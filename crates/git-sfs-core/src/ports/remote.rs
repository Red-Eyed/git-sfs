//! The remote: a second, subprocess-reached copy of the object store.
//!
//! contract-spec §5, rust-rewrite-plan §2.5/§3.3. `rclone` is the only
//! supported mover (AGENTS.md), so [`Remote`] is not a backend-abstraction
//! trait — it exists because there are genuinely two implementations,
//! [`RcloneRemote`] (real) and [`FakeRemote`] (in-memory, for higher-layer
//! tests), which is the bar rust-rewrite-plan §3.3 sets for introducing one
//! at all.
//!
//! Every rclone invocation here is retried and classified the same way, by
//! rclone's own documented exit codes (<https://rclone.org/docs/#exit-code>)
//! rather than by grepping its message text — the fix contract-spec §13.3
//! names directly for `isRemotePathNotFound`, which breaks on a wording
//! change, a localized message, or a path containing the word "config".
//! Three codes matter here: `3`/`4` ("directory/file not found", confirmed
//! absence — a normal outcome, not a failure), `5` ("temporary error, more
//! retries might fix it" — the *only* class this module retries, unlike v1's
//! `retryLoop`, which retried bad credentials and missing paths just as
//! eagerly as a network blip). Everything else is permanent.
//!
//! **`--temp-dir` is mandatory, not optional.** v1 omits it entirely from
//! push and only warns-then-proceeds when it is unset for pull
//! (`command.go:234-274`) — the exact gap that let a full system-wide `/tmp`
//! take a shared cluster's git-sfs down even though the cache itself, on a
//! separate filesystem, had room (contract-spec §13.4, [`super::store`]'s
//! module doc). [`RcloneRemote::new`] takes `temp_dir` as a required
//! argument and routes every write through it, for both directions.
//!
//! **`copy_to_remote` carries `--ignore-existing`, which v1's push omits.**
//! Without it, push overwrites an already-good remote object with whatever
//! the local copy currently is — and a locally-rotted read-only object is
//! trusted without re-hashing ([`super::store::Store::verified`]), so a
//! single bad local bit destroys the one replica that could have repaired it
//! (contract-spec §13.4: "push replicates local rot over a good remote
//! copy, and exits 0"). Adding the same flag pull already carries closes
//! this without needing push to read and hash bytes it would otherwise skip.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read as _, Write as _};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::{ALGORITHM, Sha256};
use crate::domain::remote::object_url;
use crate::error::Error;

use super::hashing;

/// How often a wait loop (for the child to exit, or for a retry backoff)
/// re-checks `cancel` — the same cadence [`super::lock::Lock`] polls
/// contention at, kept consistent across the two ports that wait on
/// something external.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Why a [`Remote`] operation failed.
#[derive(Debug, Error)]
pub enum RemoteError {
    /// The `rclone` subprocess itself could not be spawned, waited on, or
    /// have its staging files prepared — distinct from [`Failed`](Self::Failed),
    /// which means rclone ran and reported a failure of its own.
    #[error("{command}: {source}")]
    Io {
        /// What was being attempted, for diagnostics.
        command: String,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// `rclone` ran and exited reporting a failure that is not a confirmed
    /// "not found" — or a precondition git-sfs checks before ever invoking
    /// it (e.g. a configured but missing rclone config file) failed, in
    /// which case `exit_code` is `None`.
    #[error("{command} failed (exit {exit_code:?}): {message}")]
    Failed {
        /// The rclone invocation that failed, for diagnostics.
        command: String,
        /// rclone's exit code — `None` if it was killed by a signal, or if
        /// rclone was never invoked because a precondition failed first.
        exit_code: Option<i32>,
        /// Combined stdout+stderr, trimmed.
        message: String,
    },
    /// rclone succeeded but its `lsjson` output could not be parsed as the
    /// listing format git-sfs depends on.
    #[error("parsing rclone lsjson output: {source}")]
    InvalidListing {
        /// The underlying parse error.
        #[source]
        source: serde_json::Error,
    },
    /// The bytes downloaded from the remote for verification do not hash to
    /// the name they are stored under. Corrupt, not absent — contract-spec
    /// §9.1 is explicit that the two are different classes, mirroring
    /// `StoreError::HashMismatch`.
    #[error("remote object corrupt: does not hash to its own name (want {want}, got {got})")]
    HashMismatch {
        /// The hash the remote object is supposed to have.
        want: Sha256,
        /// The hash its downloaded bytes actually produce.
        got: Sha256,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

/// Runs `rclone version` and returns the detected version string, e.g.
/// `"1.67.0"`.
///
/// # Errors
///
/// Returns [`RemoteError::Failed`] if rclone cannot run or its output cannot
/// be parsed, and [`RemoteError::Canceled`] if `cancel` fires.
pub fn detect_rclone_version(cancel: &Cancel) -> Result<String, RemoteError> {
    RcloneRemote::new("", ".").detect_version(cancel)
}

impl From<RemoteError> for Error {
    fn from(err: RemoteError) -> Self {
        match err {
            RemoteError::Io { .. }
            | RemoteError::Failed { .. }
            | RemoteError::InvalidListing { .. } => Error::Unavailable(err.to_string()),
            RemoteError::HashMismatch { .. } => Error::Integrity(err.to_string()),
            RemoteError::Canceled => Error::Canceled,
        }
    }
}

/// A content-addressed remote object store, reached exclusively through
/// `rclone` subprocesses. Mirrors [`super::store::Store`]'s shape one layer
/// further out: the same hash-addressed object space, sitting behind a
/// subprocess instead of the filesystem.
pub trait Remote {
    /// Verifies the backend itself is reachable, without checking whether
    /// the configured root path exists.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError::Failed`] if the backend cannot be reached at
    /// all (bad credentials, no network, a misconfigured backend), and
    /// [`RemoteError::Canceled`] if `cancel` fires.
    fn check_backend(&self, cancel: &Cancel) -> Result<(), RemoteError>;

    /// Verifies the configured root path exists, on an already-reachable
    /// backend.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError::Failed`] if the path is confirmed absent or
    /// the listing otherwise fails, and [`RemoteError::Canceled`] if
    /// `cancel` fires.
    fn check_path(&self, cancel: &Cancel) -> Result<(), RemoteError>;

    /// [`Remote::check_backend`] then [`Remote::check_path`] — the
    /// before-push/pull preflight. A default method so implementers only
    /// write the two checks that actually vary, not their composition.
    ///
    /// # Errors
    ///
    /// Whatever the first failing check returns.
    fn require_exists(&self, cancel: &Cancel) -> Result<(), RemoteError> {
        self.check_backend(cancel)?;
        self.check_path(cancel)
    }

    /// Whether `hash` is present on the remote.
    ///
    /// Three outcomes, not two, per rust-rewrite-plan §2.5: `Ok(false)`
    /// means rclone confirmed the object is absent; `Ok(true)` means
    /// present; `Err` means the question could not be answered. A caller
    /// that cannot reach the remote must never be able to mistake that for
    /// the remote being empty — the exact defect `sizes, _ :=
    /// r.FileSizes(...)` commits in v1.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError::Failed`]/[`RemoteError::Io`] if presence could
    /// not be determined, and [`RemoteError::Canceled`] if `cancel` fires.
    fn has_file(&self, hash: Sha256, cancel: &Cancel) -> Result<bool, RemoteError>;

    /// `hash`'s size on the remote, or `None` if confirmed absent.
    ///
    /// # Errors
    ///
    /// Same as [`Remote::has_file`].
    fn file_size(&self, hash: Sha256, cancel: &Cancel) -> Result<Option<u64>, RemoteError>;

    /// Sizes of every hash in `hashes` that is present on the remote. The
    /// concrete remote may batch these queries, but it must not issue one
    /// subprocess per object or list the entire remote object store to answer
    /// a scoped question.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the listing itself could not be completed. This is
    /// the call site contract-spec §13.3's headline defect names directly —
    /// v1 turns exactly this failure into an empty map, which
    /// `status --remote`/`verify --check-remote` then report as "the remote
    /// has none of your data."
    fn file_sizes(
        &self,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError>;

    /// Uploads the objects at `rel_paths` (relative to `cache_files_dir`,
    /// e.g. `sha256/ab/ab3f...`) to the remote. Existing remote objects are
    /// never overwritten — see the module doc on why this direction carries
    /// `--ignore-existing` where v1's does not. A no-op if `rel_paths` is
    /// empty.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError::Failed`] if the copy did not complete, and
    /// [`RemoteError::Canceled`] if `cancel` fires mid-transfer.
    fn copy_to_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> Result<(), RemoteError>;

    /// Downloads the objects at `rel_paths` into `cache_files_dir`. Existing
    /// local objects are preserved. A no-op if `rel_paths` is empty.
    ///
    /// # Errors
    ///
    /// Same as [`Remote::copy_to_remote`].
    fn copy_from_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> Result<(), RemoteError>;

    /// Downloads `hash`'s remote object to a scratch location and
    /// hash-verifies it — the full-content check `verify --check-remote`/
    /// `--integrity` needs, which [`Remote::file_size`] alone cannot provide
    /// (contract-spec §9.2: a truncated object can still match on size).
    ///
    /// Three outcomes mirroring [`super::store::Store::verified`]: `Ok(true)`
    /// present and byte-verified; `Ok(false)` confirmed absent;
    /// `Err(RemoteError::HashMismatch)` present but corrupt — missing and
    /// corrupt are different classes (contract-spec §9.1), so this is never
    /// collapsed to `Ok(false)`.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError::HashMismatch`] if the downloaded bytes do not
    /// match `hash`, [`RemoteError::Failed`]/[`RemoteError::Io`] if the
    /// download itself could not be completed, and
    /// [`RemoteError::Canceled`] if `cancel` fires.
    fn verify_file(&self, hash: Sha256, cancel: &Cancel) -> Result<bool, RemoteError>;
}

/// One `lsjson` entry, trimmed to the fields git-sfs reads.
#[derive(Deserialize)]
struct LsjsonEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Size")]
    size: u64,
}

/// Parses an `lsjson` response, treating blank output the same as `[]`.
///
/// `size` is deserialized as `u64` rather than the signed type rclone emits,
/// deliberately: a negative or unparseable size on the one object path git-sfs
/// asked for is not a value to guess through (contract-spec's own
/// philosophy — see rust-rewrite-plan §2.5) — it becomes
/// [`RemoteError::InvalidListing`] instead of silently reading as "absent" or
/// "zero bytes".
fn parse_lsjson_entries(json: &str) -> Result<Vec<LsjsonEntry>, RemoteError> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(trimmed).map_err(|source| RemoteError::InvalidListing { source })
}

/// Where rclone's own documented exit code (<https://rclone.org/docs/#exit-code>)
/// places one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitClass {
    /// `3` (directory not found) or `4` (file not found): the backend was
    /// reached and confirmed the target is absent. A normal outcome, not a
    /// failure — callers turn this into `Ok(false)`/`Ok(None)`.
    NotFound,
    /// `5`: "temporary error (one that more retries might fix)". The only
    /// class [`RcloneRemote::run`] retries.
    Temporary,
    /// Every other code, including usage errors, fatal errors (bad
    /// credentials, permission denied), and a `None` from a signal kill.
    /// Retrying cannot help.
    Permanent,
}

fn classify_exit(code: Option<i32>) -> ExitClass {
    match code {
        Some(3 | 4) => ExitClass::NotFound,
        Some(5) => ExitClass::Temporary,
        _ => ExitClass::Permanent,
    }
}

/// A shell-quoted rendering of an invocation, for [`RemoteError`]'s
/// diagnostic text. The exact text is unfrozen (contract-spec: human output
/// is free) — this only needs to be readable, not to match v1's `shellQuote`
/// (`command.go:444-454`) byte for byte, though it follows the same idea.
fn describe(command: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(command.to_owned());
    for arg in args {
        if arg.is_empty()
            || arg
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'))
        {
            parts.push(format!("{arg:?}"));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

/// One completed (or canceled) `rclone` invocation, with nothing left to do
/// but classify it.
struct Invocation {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// stderr if non-empty, else stdout — the text rclone actually put a message
/// in, for [`RemoteError::Failed`].
fn combined_message(invocation: &Invocation) -> String {
    let stderr = invocation.stderr.trim();
    if !stderr.is_empty() {
        stderr.to_owned()
    } else {
        invocation.stdout.trim().to_owned()
    }
}

/// Best-effort teardown after a cancellation request: kill the child, reap
/// it, and join the reader threads so [`RcloneRemote::spawn_and_wait`] never
/// returns while a thread reading the killed child's now-closing pipes is
/// still running. Every step here is best-effort because the outcome is
/// already decided — [`RemoteError::Canceled`] — regardless of whether the
/// kill or reap individually succeeded: the child may already have been
/// exiting on its own when cancellation was noticed.
fn kill_and_drain(
    child: &mut std::process::Child,
    stdout_handle: std::thread::JoinHandle<String>,
    stderr_handle: std::thread::JoinHandle<String>,
) {
    #[allow(
        clippy::let_underscore_must_use,
        reason = "best-effort teardown after cancellation; RemoteError::Canceled is returned regardless of whether the kill itself succeeded"
    )]
    let _ = child.kill();
    #[allow(
        clippy::let_underscore_must_use,
        reason = "best-effort teardown after cancellation; the child may already have been exiting on its own when cancellation was noticed"
    )]
    let _ = child.wait();
    #[allow(
        clippy::let_underscore_must_use,
        reason = "joined only so this function does not return while the reader thread is still running; its captured content is discarded on the cancellation path"
    )]
    let _ = stdout_handle.join();
    #[allow(
        clippy::let_underscore_must_use,
        reason = "joined only so this function does not return while the reader thread is still running; its captured content is discarded on the cancellation path"
    )]
    let _ = stderr_handle.join();
}

/// Sleeps for `total`, checking `cancel` every [`POLL_INTERVAL`] so a
/// canceled retry backoff stops promptly instead of riding out the full
/// delay.
fn sleep_cancelable(total: Duration, cancel: &Cancel) -> Result<(), RemoteError> {
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if cancel.is_canceled() {
            return Err(RemoteError::Canceled);
        }
        let chunk = remaining.min(POLL_INTERVAL);
        std::thread::sleep(chunk);
        remaining -= chunk;
    }
    Ok(())
}

/// The real, `rclone`-subprocess-backed [`Remote`].
pub struct RcloneRemote {
    /// The composed rclone target, e.g. `s3:bucket/prefix` —
    /// [`crate::domain::remote::compose_remote_url`].
    url: String,
    /// `--config`, if the repository pins a non-default rclone config file.
    config: Option<Utf8PathBuf>,
    /// Where every write this remote makes is staged. **Always** the
    /// cache's own `tmp/` — see the module doc.
    temp_dir: Utf8PathBuf,
    /// Attempts for rclone's own "temporary" exit class (5) before giving
    /// up. `1` means no retries.
    retry_max: u32,
    /// Backoff before the first retry, doubling each subsequent one.
    /// Production code always uses the default; tests shrink it so a
    /// retry test does not have to sleep through real seconds.
    initial_backoff: Duration,
    /// The binary this remote invokes. Always `"rclone"` in production;
    /// tests point it at an absolute path to a fake script instead of
    /// mutating the process-global `PATH`, which is not safe under
    /// `cargo test`'s parallel execution.
    rclone_bin: String,
}

impl RcloneRemote {
    /// `url` is a pre-composed rclone target (see
    /// [`crate::domain::remote::compose_remote_url`]). `temp_dir` is
    /// mandatory — see the module doc for why v1's optional `TempDir` is not
    /// reproduced.
    #[must_use]
    pub fn new(url: impl Into<String>, temp_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            url: url.into(),
            config: None,
            temp_dir: temp_dir.into(),
            retry_max: 3,
            initial_backoff: Duration::from_secs(1),
            rclone_bin: "rclone".to_owned(),
        }
    }

    /// Sets `--config`, for a repository pinning a non-default rclone config
    /// file (contract-spec §6).
    #[must_use]
    pub fn with_config(mut self, config: impl Into<Utf8PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Overrides the default of 3 attempts for rclone's own "temporary
    /// error" exit class (5).
    #[must_use]
    pub fn with_retry_max(mut self, retry_max: u32) -> Self {
        self.retry_max = retry_max;
        self
    }

    #[cfg(test)]
    fn with_rclone_bin(mut self, bin: impl Into<String>) -> Self {
        self.rclone_bin = bin.into();
        self
    }

    #[cfg(test)]
    fn with_initial_backoff(mut self, backoff: Duration) -> Self {
        self.initial_backoff = backoff;
        self
    }

    fn files_url(&self) -> String {
        format!("{}/files", self.url)
    }

    fn object_prefix_url(&self, prefix: &str) -> String {
        format!("{}/files/{ALGORITHM}/{prefix}", self.url)
    }

    /// The bare backend prefix, e.g. `"s3:"` from `"s3:bucket/prefix"` —
    /// probed before checking a specific path, mirroring v1's
    /// `backendRoot` (`command.go:82-87`).
    fn backend_root(&self) -> String {
        match self.url.split_once(':') {
            Some((backend, _)) => format!("{backend}:"),
            None => self.url.clone(),
        }
    }

    fn detect_version(&self, cancel: &Cancel) -> Result<String, RemoteError> {
        let output = self.run(&["version".to_owned()], cancel)?;
        parse_rclone_version(&output).ok_or_else(|| RemoteError::Failed {
            command: "rclone version".to_owned(),
            exit_code: None,
            message: format!("could not parse rclone version from output: {output:?}"),
        })
    }

    /// Fails fast, before any rclone invocation, if a configured rclone
    /// config file does not exist — a wrong or missing path then produces a
    /// clear error instead of an errno surfacing deep inside a copy.
    fn validate_config(&self) -> Result<(), RemoteError> {
        let Some(config) = &self.config else {
            return Ok(());
        };
        if config.is_file() {
            Ok(())
        } else {
            Err(RemoteError::Failed {
                command: "rclone config validation".to_owned(),
                exit_code: None,
                message: format!("rclone config file not found: {config}"),
            })
        }
    }

    /// Runs `rclone` to completion, retrying only its own exit-5 "temporary
    /// error" class with exponential backoff — unlike v1's `retryLoop`,
    /// which retried every failure indiscriminately, turning a
    /// bad-credentials error into merely a slow one (contract-spec §13.4).
    /// `--config` is prepended when this remote has one, matching
    /// `newRcloneRemote`'s "must appear before any remote access" ordering.
    fn run(&self, subcommand_args: &[String], cancel: &Cancel) -> Result<String, RemoteError> {
        let mut args = Vec::with_capacity(subcommand_args.len() + 2);
        if let Some(config) = &self.config {
            args.push("--config".to_owned());
            args.push(config.to_string());
        }
        args.extend(subcommand_args.iter().cloned());

        let mut backoff = self.initial_backoff;
        let mut attempt = 1u32;
        loop {
            if cancel.is_canceled() {
                return Err(RemoteError::Canceled);
            }
            let invocation = self.spawn_and_wait(&args, cancel)?;
            if invocation.status.success() {
                return Ok(invocation.stdout);
            }
            let exit_code = invocation.status.code();
            let retryable = classify_exit(exit_code) == ExitClass::Temporary;
            if retryable && attempt < self.retry_max.max(1) {
                sleep_cancelable(backoff, cancel)?;
                backoff *= 2;
                attempt += 1;
                continue;
            }
            return Err(RemoteError::Failed {
                command: describe(&self.rclone_bin, &args),
                exit_code,
                message: combined_message(&invocation),
            });
        }
    }

    /// [`RcloneRemote::run`], but a confirmed "not found" (exit 3/4) becomes
    /// `Ok(None)` instead of an error — the shape [`Remote::has_file`]/
    /// [`Remote::file_size`]/[`Remote::file_sizes`] need, factored out once
    /// so each doesn't hand-roll the same match.
    fn run_allowing_not_found(
        &self,
        subcommand_args: &[String],
        cancel: &Cancel,
    ) -> Result<Option<String>, RemoteError> {
        match self.run(subcommand_args, cancel) {
            Ok(out) => Ok(Some(out)),
            Err(RemoteError::Failed { exit_code, .. })
                if classify_exit(exit_code) == ExitClass::NotFound =>
            {
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Spawns `rclone`, draining stdout/stderr on their own threads so a
    /// chatty child (a `copy` over many files) cannot deadlock against the
    /// cancellation-poll loop below by filling its pipe buffer before this
    /// process reads it. Polls `cancel` every [`POLL_INTERVAL`] while
    /// waiting, since — unlike [`super::cancellable_io::Cancellable`], which
    /// checks per read chunk because *we* own that loop — the byte-moving
    /// loop here belongs to the child process, not to us; killing it is the
    /// only way to stop it promptly.
    fn spawn_and_wait(&self, args: &[String], cancel: &Cancel) -> Result<Invocation, RemoteError> {
        let mut child = std::process::Command::new(&self.rclone_bin)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|source| RemoteError::Io {
                command: describe(&self.rclone_bin, args),
                source,
            })?;

        let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let stdout_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            #[allow(
                clippy::let_underscore_must_use,
                reason = "a truncated or invalid-UTF-8 capture only degrades the diagnostic message on failure, never correctness"
            )]
            let _ = stdout_pipe.read_to_string(&mut buf);
            buf
        });
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            #[allow(
                clippy::let_underscore_must_use,
                reason = "a truncated or invalid-UTF-8 capture only degrades the diagnostic message on failure, never correctness"
            )]
            let _ = stderr_pipe.read_to_string(&mut buf);
            buf
        });

        let status = loop {
            if cancel.is_canceled() {
                kill_and_drain(&mut child, stdout_handle, stderr_handle);
                return Err(RemoteError::Canceled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(source) => {
                    return Err(RemoteError::Io {
                        command: describe(&self.rclone_bin, args),
                        source,
                    });
                }
            }
        };

        let stdout = stdout_handle.join().expect("stdout reader thread panicked");
        let stderr = stderr_handle.join().expect("stderr reader thread panicked");
        Ok(Invocation {
            status,
            stdout,
            stderr,
        })
    }

    /// Stages the `--files-from` list [`Remote::copy_to_remote`]/
    /// [`Remote::copy_from_remote`] need, one relative path per line, inside
    /// this remote's own `temp_dir` — **never** the OS temp directory. v1's
    /// `writeTempPathList` falls back to `os.TempDir()` when unset
    /// (`command.go:213`); see the module doc for why that gap is not
    /// reproduced here. The returned [`tempfile::NamedTempFile`] cleans
    /// itself up on drop, so a failed or canceled copy leaves nothing behind
    /// for the caller to remember to remove.
    fn write_transfer_list(
        &self,
        rel_paths: &[Utf8PathBuf],
    ) -> Result<tempfile::NamedTempFile, RemoteError> {
        std::fs::create_dir_all(&self.temp_dir).map_err(|source| RemoteError::Io {
            command: "create rclone transfer-list directory".to_owned(),
            source,
        })?;
        let mut file = tempfile::Builder::new()
            .prefix("git-sfs-rclone-files-")
            .tempfile_in(&self.temp_dir)
            .map_err(|source| RemoteError::Io {
                command: "create rclone transfer-list file".to_owned(),
                source,
            })?;
        for path in rel_paths {
            writeln!(file, "{path}").map_err(|source| RemoteError::Io {
                command: "write rclone transfer list".to_owned(),
                source,
            })?;
        }
        file.flush().map_err(|source| RemoteError::Io {
            command: "flush rclone transfer list".to_owned(),
            source,
        })?;
        Ok(file)
    }
}

fn parse_rclone_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("rclone v")
            .and_then(|rest| rest.split_whitespace().next())
            .filter(|version| !version.is_empty())
            .map(ToOwned::to_owned)
    })
}

impl Remote for RcloneRemote {
    fn require_exists(&self, cancel: &Cancel) -> Result<(), RemoteError> {
        // Overrides the default composition to add the config-file
        // precondition first, matching v1's `RequireExists`
        // (`validateConfig` -> `CheckBackend` -> `CheckPath`).
        self.validate_config()?;
        self.check_backend(cancel)?;
        self.check_path(cancel)
    }

    fn check_backend(&self, cancel: &Cancel) -> Result<(), RemoteError> {
        match self.run(&["lsd".to_owned(), self.backend_root()], cancel) {
            Ok(_) => Ok(()),
            Err(RemoteError::Failed { exit_code, .. })
                if classify_exit(exit_code) == ExitClass::NotFound =>
            {
                // Reachable, just nothing at the root yet -- not a
                // connectivity failure.
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn check_path(&self, cancel: &Cancel) -> Result<(), RemoteError> {
        self.run(&["lsjson".to_owned(), self.url.clone()], cancel)
            .map(|_| ())
    }

    fn has_file(&self, hash: Sha256, cancel: &Cancel) -> Result<bool, RemoteError> {
        let path = object_url(&self.url, hash);
        let out = self.run_allowing_not_found(&["lsjson".to_owned(), path], cancel)?;
        match out {
            None => Ok(false),
            Some(json) => Ok(!parse_lsjson_entries(&json)?.is_empty()),
        }
    }

    fn file_size(&self, hash: Sha256, cancel: &Cancel) -> Result<Option<u64>, RemoteError> {
        let path = object_url(&self.url, hash);
        let Some(json) = self.run_allowing_not_found(&["lsjson".to_owned(), path], cancel)? else {
            return Ok(None);
        };
        let entries = parse_lsjson_entries(&json)?;
        Ok(entries.first().map(|entry| entry.size))
    }

    fn file_sizes(
        &self,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let mut result = HashMap::with_capacity(hashes.len());
        for (prefix, prefix_hashes) in hashes_by_prefix(hashes) {
            let Some(json) = self.run_allowing_not_found(
                &["lsjson".to_owned(), self.object_prefix_url(&prefix)],
                cancel,
            )?
            else {
                continue;
            };
            let entries = parse_lsjson_entries(&json)?;
            let mut wanted: HashMap<String, Sha256> =
                prefix_hashes.iter().map(|h| (h.to_hex(), *h)).collect();
            for entry in entries {
                if let Some(hash) = wanted.remove(&entry.name) {
                    result.insert(hash, entry.size);
                }
            }
        }
        Ok(result)
    }

    fn copy_to_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> Result<(), RemoteError> {
        if rel_paths.is_empty() {
            return Ok(());
        }
        let list = self.write_transfer_list(rel_paths)?;
        let list_path = list
            .path()
            .to_str()
            .expect("rclone transfer list path is UTF-8")
            .to_owned();

        self.run(
            &[
                "copy".to_owned(),
                // Skips files whose remote checksum already matches,
                // preferring the backend's own hash where it exposes one.
                "--checksum".to_owned(),
                // See the module doc: this direction lacks the flag in v1.
                "--ignore-existing".to_owned(),
                "--temp-dir".to_owned(),
                self.temp_dir.to_string(),
                "--files-from".to_owned(),
                list_path,
                cache_files_dir.to_string(),
                self.files_url(),
            ],
            cancel,
        )?;
        Ok(())
    }

    fn copy_from_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> Result<(), RemoteError> {
        if rel_paths.is_empty() {
            return Ok(());
        }
        let list = self.write_transfer_list(rel_paths)?;
        let list_path = list
            .path()
            .to_str()
            .expect("rclone transfer list path is UTF-8")
            .to_owned();

        self.run(
            &[
                "copy".to_owned(),
                "--ignore-existing".to_owned(),
                "--temp-dir".to_owned(),
                self.temp_dir.to_string(),
                "--files-from".to_owned(),
                list_path,
                self.files_url(),
                cache_files_dir.to_string(),
            ],
            cancel,
        )?;
        Ok(())
    }

    fn verify_file(&self, hash: Sha256, cancel: &Cancel) -> Result<bool, RemoteError> {
        std::fs::create_dir_all(&self.temp_dir).map_err(|source| RemoteError::Io {
            command: "create rclone verify staging directory".to_owned(),
            source,
        })?;
        let staging = tempfile::Builder::new()
            .prefix("git-sfs-rclone-verify-")
            .tempfile_in(&self.temp_dir)
            .map_err(|source| RemoteError::Io {
                command: "create rclone verify staging file".to_owned(),
                source,
            })?;
        let staging_path = staging
            .path()
            .to_str()
            .expect("rclone verify staging path is UTF-8")
            .to_owned();
        let remote_path = object_url(&self.url, hash);

        match self.run(&["copyto".to_owned(), remote_path, staging_path], cancel) {
            Ok(_) => {
                let staging_utf8 =
                    Utf8Path::from_path(staging.path()).expect("staging path is UTF-8");
                let got =
                    hashing::hash_file(staging_utf8, cancel).map_err(|source| RemoteError::Io {
                        command: "hash downloaded object".to_owned(),
                        source,
                    })?;
                if got == hash {
                    Ok(true)
                } else {
                    Err(RemoteError::HashMismatch { want: hash, got })
                }
            }
            Err(RemoteError::Failed { exit_code, .. })
                if classify_exit(exit_code) == ExitClass::NotFound =>
            {
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }
}

fn hashes_by_prefix(hashes: &[Sha256]) -> BTreeMap<String, Vec<Sha256>> {
    let mut grouped: BTreeMap<String, Vec<Sha256>> = BTreeMap::new();
    for &hash in hashes {
        grouped.entry(hash.prefix()).or_default().push(hash);
    }
    grouped
}

/// An in-memory [`Remote`], for tests above this layer that need a remote
/// without a network or an `rclone` binary. Its existence is what justifies
/// `Remote` being a trait at all (rust-rewrite-plan §3.3) — mirrors
/// [`super::store::FakeStore`]'s role for [`super::store::Store`].
///
/// Objects are keyed by the same relative path [`Remote::copy_to_remote`]
/// addresses them by (`sha256/<prefix>/<hash>`), so the local-side "files/"
/// root and this remote's object space line up exactly the way contract-spec
/// §5 requires of the real thing.
#[derive(Default)]
pub struct FakeRemote {
    objects: std::sync::Mutex<HashMap<Utf8PathBuf, Vec<u8>>>,
    /// When `false`, every operation fails as [`RemoteError::Failed`] rather
    /// than consulting `objects` — lets higher-layer tests exercise "the
    /// remote is unreachable" without a real rclone failure to provoke it,
    /// and proves this fake honors the same three-state contract
    /// [`RcloneRemote`] does.
    reachable: std::sync::atomic::AtomicBool,
}

impl FakeRemote {
    /// A reachable, empty remote.
    #[must_use]
    pub fn new() -> Self {
        Self {
            objects: std::sync::Mutex::default(),
            reachable: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Marks this remote unreachable for the rest of the test.
    pub fn set_unreachable(&self) {
        self.reachable
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn require_reachable(&self) -> Result<(), RemoteError> {
        if self.reachable.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(RemoteError::Failed {
                command: "fake remote".to_owned(),
                exit_code: None,
                message: "remote marked unreachable for this test".to_owned(),
            })
        }
    }

    fn rel_path(hash: Sha256) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash.to_hex()))
    }
}

impl Remote for FakeRemote {
    fn check_backend(&self, _cancel: &Cancel) -> Result<(), RemoteError> {
        self.require_reachable()
    }

    fn check_path(&self, _cancel: &Cancel) -> Result<(), RemoteError> {
        self.require_reachable()
    }

    fn has_file(&self, hash: Sha256, _cancel: &Cancel) -> Result<bool, RemoteError> {
        self.require_reachable()?;
        let objects = self.objects.lock().expect("fake remote mutex poisoned");
        Ok(objects.contains_key(&Self::rel_path(hash)))
    }

    fn file_size(&self, hash: Sha256, _cancel: &Cancel) -> Result<Option<u64>, RemoteError> {
        self.require_reachable()?;
        let objects = self.objects.lock().expect("fake remote mutex poisoned");
        Ok(objects
            .get(&Self::rel_path(hash))
            .map(|bytes| bytes.len() as u64))
    }

    fn file_sizes(
        &self,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError> {
        self.require_reachable()?;
        let mut result = HashMap::with_capacity(hashes.len());
        for &hash in hashes {
            if let Some(size) = self.file_size(hash, cancel)? {
                result.insert(hash, size);
            }
        }
        Ok(result)
    }

    fn copy_to_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        _cancel: &Cancel,
    ) -> Result<(), RemoteError> {
        self.require_reachable()?;
        let mut objects = self.objects.lock().expect("fake remote mutex poisoned");
        for rel in rel_paths {
            if objects.contains_key(rel) {
                continue; // --ignore-existing semantics
            }
            let bytes =
                std::fs::read(cache_files_dir.join(rel)).map_err(|source| RemoteError::Io {
                    command: format!("read {rel} for fake upload"),
                    source,
                })?;
            objects.insert(rel.clone(), bytes);
        }
        Ok(())
    }

    fn copy_from_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        _cancel: &Cancel,
    ) -> Result<(), RemoteError> {
        self.require_reachable()?;
        let objects = self.objects.lock().expect("fake remote mutex poisoned");
        for rel in rel_paths {
            let dst = cache_files_dir.join(rel);
            if dst.exists() {
                continue; // --ignore-existing semantics
            }
            let Some(bytes) = objects.get(rel) else {
                return Err(RemoteError::Failed {
                    command: format!("fake download {rel}"),
                    exit_code: Some(4),
                    message: "object not found".to_owned(),
                });
            };
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|source| RemoteError::Io {
                    command: format!("create {parent}"),
                    source,
                })?;
            }
            std::fs::write(&dst, bytes).map_err(|source| RemoteError::Io {
                command: format!("write {dst}"),
                source,
            })?;
        }
        Ok(())
    }

    fn verify_file(&self, hash: Sha256, _cancel: &Cancel) -> Result<bool, RemoteError> {
        self.require_reachable()?;
        let objects = self.objects.lock().expect("fake remote mutex poisoned");
        let Some(bytes) = objects.get(&Self::rel_path(hash)) else {
            return Ok(false);
        };
        let got = {
            use sha2::{Digest, Sha256 as Sha256Hasher};
            Sha256::from_digest(Sha256Hasher::digest(bytes).into())
        };
        if got == hash {
            Ok(true)
        } else {
            Err(RemoteError::HashMismatch { want: hash, got })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Writes an executable POSIX-sh script at `dir/rclone` that always
    /// returns the same response, regardless of its arguments — enough for
    /// every test that only needs to exercise exit-code classification, not
    /// argv handling.
    fn fixed_response_rclone(dir: &std::path::Path, stdout: &str, stderr: &str, exit_code: i32) {
        let script = format!(
            "#!/bin/sh\ncat <<'GIT_SFS_STDOUT'\n{stdout}\nGIT_SFS_STDOUT\ncat <<'GIT_SFS_STDERR' >&2\n{stderr}\nGIT_SFS_STDERR\nexit {exit_code}\n"
        );
        write_executable(&dir.join("rclone"), &script);
    }

    /// Writes an executable POSIX-sh script at `dir/rclone` that fails with
    /// `exit_code` a fixed number of times (recorded in `dir/count`, so
    /// tests can assert exactly how many attempts were made) before
    /// succeeding with `[]`.
    fn flaky_rclone(dir: &std::path::Path, failures_before_success: u32, exit_code: i32) {
        let script = format!(
            r#"#!/bin/sh
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
n=$(( $(cat "$here/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$here/count"
if [ "$n" -le {failures_before_success} ]; then
  echo "temporary failure $n" >&2
  exit {exit_code}
fi
echo '[]'
exit 0
"#
        );
        write_executable(&dir.join("rclone"), &script);
    }

    /// Writes an executable POSIX-sh script at `dir/rclone` that sleeps
    /// long enough that a test cancelling shortly after starting it proves
    /// promptness, not luck.
    fn slow_rclone(dir: &std::path::Path) {
        write_executable(
            &dir.join("rclone"),
            "#!/bin/sh\nsleep 30\necho '[]'\nexit 0\n",
        );
    }

    /// Writes an executable POSIX-sh script at `dir/rclone` that logs its
    /// full argv to `dir/argv.log` and then performs the copy `rclone copy
    /// --files-from <list> <src> <dst>` would: for each relative path in the
    /// `--files-from` list, copies `<src>/<rel>` to `<dst>/<rel>`, skipping
    /// it if `--ignore-existing` was passed and the destination already
    /// exists. Relies on `RcloneRemote::copy_to_remote`/`copy_from_remote`
    /// always placing the source and destination roots as the last two
    /// arguments, which this test module controls on both ends.
    fn copying_rclone(dir: &std::path::Path) {
        let script = r#"#!/bin/sh
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "$@" >> "$here/argv.log"

# Mirrors rclone's own local backend: everything up to the first ':' names
# the backend and is dropped, so a composed target like `local:/srv/data`
# addresses `/srv/data` on this filesystem.
strip_backend() {
  case "$1" in
    *:*) printf '%s' "${1#*:}" ;;
    *) printf '%s' "$1" ;;
  esac
}

files_from=""
ignore_existing=0
while [ "$#" -gt 2 ]; do
  case "$1" in
    --files-from) files_from="$2"; shift 2 ;;
    --ignore-existing) ignore_existing=1; shift ;;
    --temp-dir) shift 2 ;;
    --config) shift 2 ;;
    *) shift ;;
  esac
done
src_root=$(strip_backend "$1")
dst_root=$(strip_backend "$2")

while IFS= read -r rel; do
  [ -z "$rel" ] && continue
  src="$src_root/$rel"
  dst="$dst_root/$rel"
  if [ "$ignore_existing" = 1 ] && [ -e "$dst" ]; then
    continue
  fi
  mkdir -p "$(dirname "$dst")"
  cp "$src" "$dst"
done < "$files_from"

echo '[]'
exit 0
"#;
        write_executable(&dir.join("rclone"), script);
    }

    /// Writes an executable POSIX-sh script at `dir/rclone` that logs its
    /// full argv and implements just enough `lsjson` to prove which remote
    /// directory `file_sizes` listed.
    fn listing_rclone(dir: &std::path::Path) {
        let script = r#"#!/bin/sh
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "$@" >> "$here/argv.log"

strip_backend() {
  case "$1" in
    *:*) printf '%s' "${1#*:}" ;;
    *) printf '%s' "$1" ;;
  esac
}

target=""
for arg do
  target="$arg"
done
root=$(strip_backend "$target")

if [ ! -e "$root" ]; then
  echo "directory not found: $root" >&2
  exit 3
fi

if [ -f "$root" ]; then
  name=$(basename "$root")
  size=$(wc -c < "$root" | tr -d ' ')
  printf '[{"Name":"%s","Size":%s}]\n' "$name" "$size"
  exit 0
fi

first=1
printf '['
for path in "$root"/*; do
  [ -e "$path" ] || continue
  [ -f "$path" ] || continue
  name=$(basename "$path")
  size=$(wc -c < "$path" | tr -d ' ')
  if [ "$first" = 1 ]; then
    first=0
  else
    printf ','
  fi
  printf '{"Name":"%s","Size":%s}' "$name" "$size"
done
printf ']\n'
exit 0
"#;
        write_executable(&dir.join("rclone"), script);
    }

    fn write_executable(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn remote_at(fixture: &std::path::Path, temp_dir: &std::path::Path) -> RcloneRemote {
        RcloneRemote::new(
            "local:/wherever",
            Utf8PathBuf::from_path_buf(temp_dir.to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.join("rclone").to_str().unwrap().to_owned())
        .with_initial_backoff(Duration::from_millis(5))
    }

    fn a_hash() -> Sha256 {
        Sha256::parse("ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a").unwrap()
    }

    fn c_hash() -> Sha256 {
        Sha256::parse("cd3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a").unwrap()
    }

    #[test]
    fn has_file_returns_true_when_rclone_reports_the_object_present() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), r#"[{"Name":"x","Size":42}]"#, "", 0);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert!(remote.has_file(a_hash(), &Cancel::new()).unwrap());
    }

    #[test]
    fn has_file_returns_false_when_rclone_confirms_not_found() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), "", "directory not found", 3);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert!(!remote.has_file(a_hash(), &Cancel::new()).unwrap());
    }

    /// The headline test for rust-rewrite-plan §2.5: an unreachable remote
    /// must surface as `Err`, never silently collapse to "the object is
    /// absent" the way v1's `HasFile`/`FileSizes` do.
    #[test]
    fn has_file_returns_an_error_when_the_remote_cannot_be_reached() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), "", "fatal error: invalid credentials", 7);
        let remote = remote_at(fixture.path(), temp_dir.path());

        let err = remote.has_file(a_hash(), &Cancel::new()).unwrap_err();
        assert!(matches!(
            err,
            RemoteError::Failed {
                exit_code: Some(7),
                ..
            }
        ));
    }

    #[test]
    fn file_size_distinguishes_absent_from_present() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), r#"[{"Name":"x","Size":123}]"#, "", 0);
        let remote = remote_at(fixture.path(), temp_dir.path());
        assert_eq!(
            remote.file_size(a_hash(), &Cancel::new()).unwrap(),
            Some(123)
        );
    }

    #[test]
    fn file_sizes_lists_requested_prefixes_instead_of_the_entire_remote() {
        let fixture = tempfile::tempdir().unwrap();
        let remote_root = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        listing_rclone(fixture.path());

        let ab_hash = a_hash();
        let cd_hash = c_hash();
        let ab_path =
            remote_root
                .path()
                .join(format!("files/sha256/{}/{}", ab_hash.prefix(), ab_hash));
        let cd_path =
            remote_root
                .path()
                .join(format!("files/sha256/{}/{}", cd_hash.prefix(), cd_hash));
        std::fs::create_dir_all(ab_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(cd_path.parent().unwrap()).unwrap();
        std::fs::write(&ab_path, b"abc").unwrap();
        std::fs::write(&cd_path, b"longer").unwrap();

        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.path().display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let sizes = remote
            .file_sizes(&[ab_hash, cd_hash], &Cancel::new())
            .unwrap();

        assert_eq!(sizes.get(&ab_hash), Some(&3));
        assert_eq!(sizes.get(&cd_hash), Some(&6));

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        let remote_root_url = format!("local:{}", remote_root.path().display());
        assert!(
            log.contains(&format!("lsjson {remote_root_url}/files/sha256/ab")),
            "argv: {log}"
        );
        assert!(
            log.contains(&format!("lsjson {remote_root_url}/files/sha256/cd")),
            "argv: {log}"
        );
        assert!(
            !log.contains(&format!("lsjson --recursive {remote_root_url}/files")),
            "file_sizes must not list the whole remote object tree, argv: {log}"
        );
    }

    #[test]
    fn retries_only_rclones_own_temporary_exit_class_and_gives_up_once_exhausted() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        // Fails (exit 5, "temporary") twice, then succeeds on the 3rd try --
        // exactly what retry_max's default of 3 total attempts allows.
        flaky_rclone(fixture.path(), 2, 5);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert!(!remote.has_file(a_hash(), &Cancel::new()).unwrap());
        let count: u32 = std::fs::read_to_string(fixture.path().join("count"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(count, 3, "must retry exit 5 up to retry_max attempts");
    }

    #[test]
    fn a_permanent_failure_is_attempted_exactly_once_despite_retry_max() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        // exit 7 ("fatal error") is never in ExitClass::Temporary, so no
        // amount of retry_max should cause a second attempt -- the fix for
        // v1's retryLoop retrying bad credentials as if they were a
        // network blip (contract-spec §13.4).
        flaky_rclone(fixture.path(), 10, 7);
        let remote = remote_at(fixture.path(), temp_dir.path());

        let err = remote.has_file(a_hash(), &Cancel::new()).unwrap_err();
        assert!(matches!(
            err,
            RemoteError::Failed {
                exit_code: Some(7),
                ..
            }
        ));
        let count: u32 = std::fs::read_to_string(fixture.path().join("count"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(count, 1, "a permanent failure must not be retried");
    }

    #[test]
    fn cancellation_stops_promptly_even_mid_subprocess() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        slow_rclone(fixture.path());
        let remote = remote_at(fixture.path(), temp_dir.path());
        let cancel = Cancel::new();

        let cancel_for_worker = cancel.clone();
        let handle = std::thread::spawn(move || remote.has_file(a_hash(), &cancel_for_worker));

        std::thread::sleep(Duration::from_millis(150));
        cancel.cancel();

        let started = std::time::Instant::now();
        let result = handle
            .join()
            .expect("worker thread should not panic")
            .expect_err("a canceled call must return Err");
        assert!(matches!(result, RemoteError::Canceled));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "cancellation must not wait for the full 30s sleep"
        );
    }

    #[test]
    fn copy_to_remote_moves_bytes_and_never_overwrites_an_existing_remote_object() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let files_dir = cache.path().join("files");
        std::fs::create_dir_all(files_dir.join("sha256/ab")).unwrap();
        std::fs::write(files_dir.join("sha256/ab/hash1"), b"object bytes").unwrap();
        // `remote_root` is the composed URL's target; `files_url()` appends
        // its own "/files", so the pre-existing object must live one level
        // deeper than `remote_root` itself, matching what a real remote's
        // `<url>/files/...` layout (contract-spec §5) resolves to.
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(remote_root.join("files/sha256/ab")).unwrap();
        // Already present on the "remote" -- must survive untouched.
        std::fs::write(remote_root.join("files/sha256/ab/hash1"), b"already good").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let files_dir_utf8 = Utf8PathBuf::from_path_buf(files_dir.clone()).unwrap();
        let rel = Utf8PathBuf::from("sha256/ab/hash1");
        remote
            .copy_to_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        assert_eq!(
            std::fs::read(remote_root.join("files/sha256/ab/hash1")).unwrap(),
            b"already good",
            "--ignore-existing must prevent overwriting a good remote copy with local rot"
        );

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert!(log.contains("--ignore-existing"), "argv: {log}");
        assert!(log.contains("--checksum"), "argv: {log}");
        assert!(log.contains("--temp-dir"), "argv: {log}");
        assert!(
            log.contains(temp_dir.path().to_str().unwrap()),
            "--temp-dir must point at this remote's own temp_dir, argv: {log}"
        );
    }

    #[test]
    fn copy_from_remote_preserves_an_existing_local_object() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let files_dir = cache.path().join("files");
        std::fs::create_dir_all(files_dir.join("sha256/ab")).unwrap();
        // Already present locally -- must survive untouched.
        std::fs::write(files_dir.join("sha256/ab/hash1"), b"local original").unwrap();
        // See the sibling copy_to_remote test for why the object lives
        // under `remote_root/files/...`, not `remote_root/...` directly.
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(remote_root.join("files/sha256/ab")).unwrap();
        std::fs::write(remote_root.join("files/sha256/ab/hash1"), b"remote content").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let files_dir_utf8 = Utf8PathBuf::from_path_buf(files_dir.clone()).unwrap();
        let rel = Utf8PathBuf::from("sha256/ab/hash1");
        remote
            .copy_from_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        assert_eq!(
            std::fs::read(files_dir.join("sha256/ab/hash1")).unwrap(),
            b"local original"
        );
        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert!(log.contains("--ignore-existing"), "argv: {log}");
    }

    #[test]
    fn a_missing_transfer_list_is_a_noop() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let remote = remote_at(fixture.path(), temp_dir.path());
        // No rclone script written at all -- a real invocation would fail
        // to spawn, proving the empty-list short-circuit never shells out.
        let cache = tempfile::tempdir().unwrap();
        let cache_dir = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        remote
            .copy_to_remote(&cache_dir, &[], &Cancel::new())
            .unwrap();
        remote
            .copy_from_remote(&cache_dir, &[], &Cancel::new())
            .unwrap();
    }

    #[test]
    fn fake_remote_an_absent_object_is_false_not_an_error() {
        let remote = FakeRemote::new();
        assert!(!remote.has_file(a_hash(), &Cancel::new()).unwrap());
    }

    #[test]
    fn fake_remote_copy_round_trips_bytes_and_respects_ignore_existing() {
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let files_dir = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        let rel = Utf8PathBuf::from("sha256/ab/hash1");
        std::fs::create_dir_all(files_dir.join("sha256/ab")).unwrap();
        std::fs::write(files_dir.join(&rel), b"uploaded bytes").unwrap();

        remote
            .copy_to_remote(&files_dir, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        let download_dir = tempfile::tempdir().unwrap();
        let download_dir_utf8 = Utf8PathBuf::from_path_buf(download_dir.path().to_owned()).unwrap();
        remote
            .copy_from_remote(
                &download_dir_utf8,
                std::slice::from_ref(&rel),
                &Cancel::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(download_dir.path().join("sha256/ab/hash1")).unwrap(),
            b"uploaded bytes"
        );

        // Re-uploading different content under the same rel path must not
        // overwrite the already-present remote object.
        std::fs::write(files_dir.join(&rel), b"different content").unwrap();
        remote
            .copy_to_remote(&files_dir, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();
        let redownload_dir = tempfile::tempdir().unwrap();
        let redownload_dir_utf8 =
            Utf8PathBuf::from_path_buf(redownload_dir.path().to_owned()).unwrap();
        remote
            .copy_from_remote(
                &redownload_dir_utf8,
                std::slice::from_ref(&rel),
                &Cancel::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(redownload_dir.path().join("sha256/ab/hash1")).unwrap(),
            b"uploaded bytes",
            "--ignore-existing semantics: the original remote object must survive"
        );
    }

    #[test]
    fn fake_remote_verify_file_reports_corrupt_not_absent_for_tampered_bytes() {
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let files_dir = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        // Must land at the exact rel path `verify_file` looks up for
        // `a_hash()` -- an arbitrary path would make the object simply
        // unfindable and the test would pass for the wrong reason (Ok(false)
        // from "absent", not from a genuine mismatch check).
        let rel = FakeRemote::rel_path(a_hash());
        std::fs::create_dir_all(files_dir.join(rel.parent().unwrap())).unwrap();
        std::fs::write(files_dir.join(&rel), b"tampered on the way in").unwrap();
        remote
            .copy_to_remote(&files_dir, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        // `a_hash()` does not match the uploaded bytes' real hash, so this
        // must be reported as corrupt, not merely absent.
        let err = remote
            .verify_file(a_hash(), &Cancel::new())
            .expect_err("mismatched content must be an error, not Ok(false)");
        assert!(matches!(err, RemoteError::HashMismatch { .. }));
    }

    #[test]
    fn fake_remote_marking_unreachable_makes_every_operation_fail_rather_than_report_empty() {
        let remote = FakeRemote::new();
        remote.set_unreachable();
        let err = remote.has_file(a_hash(), &Cancel::new()).unwrap_err();
        assert!(matches!(err, RemoteError::Failed { .. }));
    }

    #[test]
    fn parses_the_first_rclone_version_line() {
        let output = "rclone v1.67.0\n- os/version: darwin\n";

        assert_eq!(parse_rclone_version(output), Some("1.67.0".to_owned()));
    }

    #[test]
    fn rejects_rclone_version_output_without_a_version_line() {
        assert_eq!(parse_rclone_version("not rclone\n"), None);
    }
}
