//! The remote: a second, subprocess-reached copy of the object store.
//!
//! Every rclone invocation here is retried and classified the same way, by
//! rclone's own documented exit codes (<https://rclone.org/docs/#exit-code>)
//! rather than by grepping its message text, which is brittle under wording
//! changes, localized output, or paths containing error-like words.
//! Three codes matter here: `3`/`4` ("directory/file not found", classified
//! separately so the backend diagnostic can distinguish an empty root), `5`
//! ("temporary error, more retries might fix it" — the *only* class this module
//! retries), and everything else, which is permanent.
//!
//! **`--temp-dir` is mandatory.** [`RcloneRemote::new`] takes `temp_dir` as a
//! required argument and routes every write through it, for both directions.
//! The OS-wide temp directory is never part of remote transfer staging.
//!
//! **Remote writes are staged, then published.** `copy_to_remote` uploads the
//! whole requested set with one `rclone copy --files-from`, but its destination
//! is a unique remote `tmp/` prefix, never the final content-addressed object
//! path. The staging namespace is per user rather than per process, so a
//! failed push can resume staged objects on the next run while different users
//! do not write into one global temp area. Only after that batch completes and
//! staged sizes match does git-sfs publish the staged objects with one batched
//! `rclone move --files-from`. This preserves rclone batching while keeping
//! interrupted uploads out of the final object namespace.

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::{ALGORITHM, Sha256};
use crate::error::Error;

/// How often a wait loop (for the child to exit, or for a retry backoff)
/// re-checks `cancel` — the same cadence [`super::lock::Lock`] polls
/// contention at, kept consistent across the two ports that wait on
/// something external.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Rclone's progress stream is mirrored live; failures only need enough tail
/// output to show the final error without retaining every terminal refresh.
const TRANSFER_DIAGNOSTIC_LIMIT: usize = 64 * 1024;
/// Rclone formats its progress display for 80 columns by default. A nonzero
/// row count is also required for its terminal-size probe to succeed.
const PROGRESS_TERMINAL_SIZE: nix::pty::Winsize = nix::pty::Winsize {
    ws_row: 24,
    ws_col: 80,
    ws_xpixel: 0,
    ws_ypixel: 0,
};

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
    /// A remote object was present but its size did not match the local
    /// object being uploaded. Size is the cheap invariant available on every
    /// rclone backend; full hash verification stays opt-in where it requires
    /// reading all bytes.
    #[error("remote object size mismatch for {hash} at {location}: want {want} bytes, got {got}")]
    SizeMismatch {
        /// The content-addressed object.
        hash: Sha256,
        /// Where the mismatch was observed.
        location: String,
        /// Expected byte count.
        want: u64,
        /// Actual byte count, or absent.
        got: RemoteSize,
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
            RemoteError::SizeMismatch { .. } => Error::Integrity(err.to_string()),
            RemoteError::Canceled => Error::Canceled,
        }
    }
}

/// Size observed for a remote object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteSize {
    /// The object was not listed.
    Absent,
    /// The object was listed with this many bytes.
    Bytes(u64),
}

impl std::fmt::Display for RemoteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => f.write_str("absent"),
            Self::Bytes(size) => write!(f, "{size} bytes"),
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

    /// Sizes of every hash in `hashes` that is present on the remote. The
    /// concrete remote must batch the complete query: it may neither issue one
    /// subprocess per object nor list the entire remote object store to answer
    /// a scoped question.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the listing itself could not be completed. This is
    /// distinct from a successful listing that simply does not include a
    /// requested object.
    fn file_sizes(
        &self,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError>;

    /// Uploads the objects at `rel_paths` (relative to `cache_files_dir`,
    /// e.g. `sha256/ab/ab3f...`) to the remote. Existing remote objects are
    /// never overwritten. A no-op if `rel_paths` is
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
/// `size` is deserialized as `u64` deliberately: a negative or unparseable
/// size on an object path git-sfs asked for becomes
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
    /// reached and confirmed the target is absent. The backend diagnostic
    /// treats this as reachable-but-empty; scoped metadata and transfers still
    /// surface it as a failed operation.
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
/// diagnostic text. This only needs to be readable; it is not a stable output
/// format.
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

#[derive(Clone, Copy)]
enum Mirror {
    None,
    Stdout,
    Stderr,
}

#[derive(Clone, Copy)]
enum Capture {
    Full,
    Tail(usize),
}

#[derive(Clone, Copy)]
struct PipeMode {
    stdout_mirror: Mirror,
    stderr_mirror: Mirror,
    stdout_capture: Capture,
    stderr_capture: Capture,
    stdout_terminal: bool,
}

struct PipeCapture {
    bytes: Vec<u8>,
    truncated: bool,
    mode: Capture,
}

impl PipeCapture {
    fn new(mode: Capture) -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
            mode,
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        let Capture::Tail(limit) = self.mode else {
            return;
        };
        let excess = self.bytes.len().saturating_sub(limit);
        if excess == 0 {
            return;
        }
        self.bytes.drain(..excess);
        self.truncated = true;
    }

    fn into_string(self) -> String {
        let text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            format!("[rclone output truncated]\n{text}")
        } else {
            text
        }
    }
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

fn read_pipe(mut pipe: impl io::Read, mirror: Mirror, capture: Capture) -> String {
    let mut captured = PipeCapture::new(capture);
    let mut chunk = [0; 8192];

    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                mirror_bytes(mirror, &chunk[..n]);
                captured.extend(&chunk[..n]);
            }
            Err(_) => break,
        }
    }

    captured.into_string()
}

fn mirror_bytes(mirror: Mirror, bytes: &[u8]) {
    match mirror {
        Mirror::None => {}
        Mirror::Stdout => {
            write_best_effort(io::stdout().lock(), bytes);
        }
        Mirror::Stderr => {
            write_best_effort(io::stderr().lock(), bytes);
        }
    }
}

fn write_best_effort(mut out: impl io::Write, bytes: &[u8]) {
    let _result = out.write_all(bytes).and_then(|()| out.flush());
}

fn open_progress_terminal() -> Result<nix::pty::OpenptyResult, RemoteError> {
    nix::pty::openpty(
        Some(&PROGRESS_TERMINAL_SIZE),
        None::<&nix::sys::termios::Termios>,
    )
    .map_err(|source| RemoteError::Io {
        command: "create rclone progress terminal".to_owned(),
        source: io::Error::from(source),
    })
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
    transfer_progress: bool,
}

impl RcloneRemote {
    /// `url` is a pre-composed rclone target (see
    /// [`crate::domain::remote::compose_remote_url`]). `temp_dir` is mandatory.
    #[must_use]
    pub fn new(url: impl Into<String>, temp_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            url: url.into(),
            config: None,
            temp_dir: temp_dir.into(),
            retry_max: 3,
            initial_backoff: Duration::from_secs(1),
            rclone_bin: "rclone".to_owned(),
            transfer_progress: false,
        }
    }

    /// Sets `--config`, for a repository pinning a non-default rclone config
    /// file.
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

    /// Mirrors rclone's own transfer progress during copy/move operations
    /// while still retaining bounded diagnostics for failures.
    #[must_use]
    pub fn with_transfer_progress(mut self, enabled: bool) -> Self {
        self.transfer_progress = enabled;
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

    fn staged_files_path(&self) -> Utf8PathBuf {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .map(|value| sanitize_stage_component(&value))
            .unwrap_or_else(|_| "unknown".to_owned());
        Utf8PathBuf::from(format!("tmp/{user}/files"))
    }

    fn staged_files_url(&self, staged_files_path: &Utf8Path) -> String {
        format!("{}/{}", self.url, staged_files_path)
    }

    /// The bare backend prefix, e.g. `"s3:"` from `"s3:bucket/prefix"`.
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
    /// error" class with exponential backoff. Permanent failures are returned
    /// immediately. `--config` is prepended when this remote has one so it
    /// applies to every remote access.
    fn run(&self, subcommand_args: &[String], cancel: &Cancel) -> Result<String, RemoteError> {
        self.run_with_output(
            subcommand_args,
            cancel,
            PipeMode {
                stdout_mirror: Mirror::None,
                stderr_mirror: Mirror::None,
                stdout_capture: Capture::Full,
                stderr_capture: Capture::Full,
                stdout_terminal: false,
            },
        )
    }

    fn run_transfer(
        &self,
        subcommand_args: &[String],
        cancel: &Cancel,
    ) -> Result<String, RemoteError> {
        let mut args = subcommand_args.to_vec();
        if self.transfer_progress {
            args.insert(args.len().min(1), "--progress".to_owned());
        }
        let capture = if self.transfer_progress {
            Capture::Tail(TRANSFER_DIAGNOSTIC_LIMIT)
        } else {
            Capture::Full
        };
        self.run_with_output(
            &args,
            cancel,
            PipeMode {
                stdout_mirror: if self.transfer_progress {
                    Mirror::Stdout
                } else {
                    Mirror::None
                },
                stderr_mirror: if self.transfer_progress {
                    Mirror::Stderr
                } else {
                    Mirror::None
                },
                stdout_capture: capture,
                stderr_capture: capture,
                stdout_terminal: self.transfer_progress,
            },
        )
    }

    fn run_with_output(
        &self,
        subcommand_args: &[String],
        cancel: &Cancel,
        pipe_mode: PipeMode,
    ) -> Result<String, RemoteError> {
        self.validate_config()?;
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
            let invocation = self.spawn_and_wait(&args, cancel, pipe_mode)?;
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

    /// Spawns `rclone`, draining either piped stdout or a progress terminal and
    /// piped stderr on their own threads. Rclone writes its cursor-redrawing
    /// progress display to stdout and truncates it to that terminal's reported
    /// width, so the progress path needs a sized stdout TTY. Reading the
    /// terminal's master side still lets us retain failure diagnostics.
    /// Concurrent draining prevents a chatty child from deadlocking against
    /// the cancellation-poll loop below by filling an output buffer. Polls
    /// `cancel` every [`POLL_INTERVAL`] while waiting, since — unlike
    /// [`super::cancellable_io::Cancellable`], which checks per read chunk
    /// because *we* own that loop — the byte-moving loop here belongs to the
    /// child process, not to us; killing it is the only way to stop it
    /// promptly.
    fn spawn_and_wait(
        &self,
        args: &[String],
        cancel: &Cancel,
        pipe_mode: PipeMode,
    ) -> Result<Invocation, RemoteError> {
        let mut command = std::process::Command::new(&self.rclone_bin);
        command.args(args).stdin(std::process::Stdio::null());
        let stdout_master = if pipe_mode.stdout_terminal {
            let terminal = open_progress_terminal()?;
            command.stdout(std::process::Stdio::from(terminal.slave));
            Some(std::fs::File::from(terminal.master))
        } else {
            command.stdout(std::process::Stdio::piped());
            None
        };
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|source| RemoteError::Io {
            command: describe(&self.rclone_bin, args),
            source,
        })?;
        drop(command);

        let mut stdout_pipe: Box<dyn io::Read + Send> = match stdout_master {
            Some(master) => Box::new(master),
            None => Box::new(child.stdout.take().expect("stdout was piped")),
        };
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
        let stdout_handle = std::thread::spawn(move || {
            read_pipe(
                &mut stdout_pipe,
                pipe_mode.stdout_mirror,
                pipe_mode.stdout_capture,
            )
        });
        let stderr_handle = std::thread::spawn(move || {
            read_pipe(
                &mut stderr_pipe,
                pipe_mode.stderr_mirror,
                pipe_mode.stderr_capture,
            )
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

    /// Stages a batched `--files-from` list, one relative path per line, inside
    /// this remote's own `temp_dir` — **never** the OS temp directory. Metadata
    /// queries and transfers share this path-list boundary so neither can drift
    /// back to one subprocess per object. The returned
    /// [`tempfile::NamedTempFile`] cleans
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

    fn file_sizes_at(
        &self,
        files_root: &Utf8Path,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let rel_paths = hashes
            .iter()
            .map(|hash| {
                files_root
                    .join(ALGORITHM)
                    .join(hash.prefix())
                    .join(hash.to_hex())
            })
            .collect::<Vec<_>>();
        let list = self.write_transfer_list(&rel_paths)?;
        let list_path = list
            .path()
            .to_str()
            .expect("rclone metadata list path is UTF-8")
            .to_owned();
        let json = self.run(
            &[
                "lsjson".to_owned(),
                "--recursive".to_owned(),
                "--files-only".to_owned(),
                "--no-mimetype".to_owned(),
                "--no-modtime".to_owned(),
                "--disable".to_owned(),
                "ListR".to_owned(),
                "--files-from".to_owned(),
                list_path,
                self.url.clone(),
            ],
            cancel,
        )?;
        let mut wanted = hashes
            .iter()
            .map(|hash| (hash.to_hex(), *hash))
            .collect::<HashMap<_, _>>();
        let mut result = HashMap::with_capacity(hashes.len());
        for entry in parse_lsjson_entries(&json)? {
            if let Some(hash) = wanted.remove(&entry.name) {
                result.insert(hash, entry.size);
            }
        }
        Ok(result)
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

    fn file_sizes(
        &self,
        hashes: &[Sha256],
        cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError> {
        self.file_sizes_at(Utf8Path::new("files"), hashes, cancel)
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
        let planned = plan_upload(cache_files_dir, rel_paths)?;
        let hashes = planned.iter().map(|object| object.hash).collect::<Vec<_>>();
        let final_sizes = self.file_sizes_at(Utf8Path::new("files"), &hashes, cancel)?;

        let upload = planned
            .into_iter()
            .filter_map(|object| match final_sizes.get(&object.hash).copied() {
                Some(size) if size == object.size => None,
                Some(size) => Some(Err(RemoteError::SizeMismatch {
                    hash: object.hash,
                    location: "remote final object".to_owned(),
                    want: object.size,
                    got: RemoteSize::Bytes(size),
                })),
                None => Some(Ok(object)),
            })
            .collect::<Result<Vec<_>, _>>()?;

        if upload.is_empty() {
            return Ok(());
        }

        let upload_rel_paths = upload
            .iter()
            .map(|object| object.rel_path.clone())
            .collect::<Vec<_>>();
        let upload_hashes = upload.iter().map(|object| object.hash).collect::<Vec<_>>();
        let staged_files_path = self.staged_files_path();
        let staged_url = self.staged_files_url(&staged_files_path);

        let list = self.write_transfer_list(&upload_rel_paths)?;
        let list_path = list
            .path()
            .to_str()
            .expect("rclone transfer list path is UTF-8")
            .to_owned();

        self.run_transfer(
            &[
                "copy".to_owned(),
                "--checksum".to_owned(),
                "--temp-dir".to_owned(),
                self.temp_dir.to_string(),
                "--files-from".to_owned(),
                list_path,
                cache_files_dir.to_string(),
                staged_url.clone(),
            ],
            cancel,
        )?;
        assert_remote_sizes(
            &upload,
            &self.file_sizes_at(&staged_files_path, &upload_hashes, cancel)?,
            "remote staged object",
        )?;

        let publish_list = self.write_transfer_list(&upload_rel_paths)?;
        let publish_list_path = publish_list
            .path()
            .to_str()
            .expect("rclone publish list path is UTF-8")
            .to_owned();
        self.run_transfer(
            &[
                "move".to_owned(),
                "--ignore-existing".to_owned(),
                "--temp-dir".to_owned(),
                self.temp_dir.to_string(),
                "--files-from".to_owned(),
                publish_list_path,
                staged_url,
                self.files_url(),
            ],
            cancel,
        )?;
        assert_remote_sizes(
            &upload,
            &self.file_sizes_at(Utf8Path::new("files"), &upload_hashes, cancel)?,
            "remote final object",
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

        self.run_transfer(
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UploadObject {
    hash: Sha256,
    rel_path: Utf8PathBuf,
    size: u64,
}

fn plan_upload(
    cache_files_dir: &Utf8Path,
    rel_paths: &[Utf8PathBuf],
) -> Result<Vec<UploadObject>, RemoteError> {
    rel_paths
        .iter()
        .map(|rel_path| upload_object(cache_files_dir, rel_path))
        .collect()
}

fn upload_object(
    cache_files_dir: &Utf8Path,
    rel_path: &Utf8Path,
) -> Result<UploadObject, RemoteError> {
    let hash = hash_from_rel_path(rel_path)?;
    let path = cache_files_dir.join(rel_path);
    let size = std::fs::metadata(&path)
        .map_err(|source| RemoteError::Io {
            command: format!("stat local object before upload: {path}"),
            source,
        })?
        .len();
    Ok(UploadObject {
        hash,
        rel_path: rel_path.to_owned(),
        size,
    })
}

fn hash_from_rel_path(rel_path: &Utf8Path) -> Result<Sha256, RemoteError> {
    let mut components = rel_path.components();
    let Some(algorithm) = components.next().map(|component| component.as_str()) else {
        return invalid_rel_path(rel_path);
    };
    let Some(prefix) = components.next().map(|component| component.as_str()) else {
        return invalid_rel_path(rel_path);
    };
    let Some(hash_text) = components.next().map(|component| component.as_str()) else {
        return invalid_rel_path(rel_path);
    };
    if components.next().is_some() || algorithm != ALGORITHM {
        return invalid_rel_path(rel_path);
    }
    let hash = Sha256::parse(hash_text).map_err(|err| RemoteError::Failed {
        command: "validate upload path".to_owned(),
        exit_code: None,
        message: format!("invalid upload path {rel_path}: {err}"),
    })?;
    if hash.prefix() != prefix {
        return invalid_rel_path(rel_path);
    }
    Ok(hash)
}

fn invalid_rel_path<T>(rel_path: &Utf8Path) -> Result<T, RemoteError> {
    Err(RemoteError::Failed {
        command: "validate upload path".to_owned(),
        exit_code: None,
        message: format!("invalid upload path: {rel_path}"),
    })
}

fn assert_remote_sizes(
    expected: &[UploadObject],
    actual: &HashMap<Sha256, u64>,
    location: &str,
) -> Result<(), RemoteError> {
    for object in expected {
        match actual.get(&object.hash).copied() {
            Some(size) if size == object.size => {}
            Some(size) => {
                return Err(RemoteError::SizeMismatch {
                    hash: object.hash,
                    location: location.to_owned(),
                    want: object.size,
                    got: RemoteSize::Bytes(size),
                });
            }
            None => {
                return Err(RemoteError::SizeMismatch {
                    hash: object.hash,
                    location: location.to_owned(),
                    want: object.size,
                    got: RemoteSize::Absent,
                });
            }
        }
    }
    Ok(())
}

fn sanitize_stage_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

/// An in-memory [`Remote`], for tests above this layer that need a remote
/// without a network or an `rclone` binary.
///
/// Objects are keyed by the same relative path [`Remote::copy_to_remote`]
/// addresses them by (`sha256/<prefix>/<hash>`), so the local-side `files/`
/// root and this remote's object space line up with the real remote layout.
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

    fn file_sizes(
        &self,
        hashes: &[Sha256],
        _cancel: &Cancel,
    ) -> Result<HashMap<Sha256, u64>, RemoteError> {
        self.require_reachable()?;
        let objects = self.objects.lock().expect("fake remote mutex poisoned");
        let mut result = HashMap::with_capacity(hashes.len());
        for &hash in hashes {
            if let Some(bytes) = objects.get(&Self::rel_path(hash)) {
                result.insert(hash, bytes.len() as u64);
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
        write_executable(&dir.join("rclone"), "#!/bin/sh\nexec sleep 30\n");
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
if [ -t 1 ]; then
  stty size <&1 > "$here/stdout-size.log"
  size=$(cat "$here/stdout-size.log")
  echo "terminal $size" >> "$here/stdout-mode.log"
else
  echo pipe >> "$here/stdout-mode.log"
fi
if [ -t 2 ]; then
  echo terminal >> "$here/stderr-mode.log"
else
  echo pipe >> "$here/stderr-mode.log"
fi

strip_backend() {
  case "$1" in
    *:*) printf '%s' "${1#*:}" ;;
    *) printf '%s' "$1" ;;
  esac
}

describe_file() {
  name=$(basename "$1")
  size=$(wc -c < "$1" | tr -d ' ')
  printf '{"Name":"%s","Size":%s}' "$name" "$size"
}

lsjson() {
  files_from=""
  while [ "$#" -gt 1 ]; do
    case "$1" in
      --files-from) files_from="$2"; shift 2 ;;
      --disable) shift 2 ;;
      *) shift ;;
    esac
  done
  target="$1"
  root=$(strip_backend "$target")
  if [ ! -d "$root" ]; then
    echo "directory not found: $root" >&2
    exit 3
  fi
  first=1
  printf '['
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    path="$root/$rel"
    [ -f "$path" ] || continue
    if [ "$first" = 1 ]; then
      first=0
    else
      printf ','
    fi
    describe_file "$path"
  done < "$files_from"
  printf ']\n'
  exit 0
}

cmd="$1"
shift
if [ "$cmd" = "lsjson" ]; then
  lsjson "$@"
fi

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
  if [ "$cmd" = "move" ]; then
    rm -f "$src"
  fi
done < "$files_from"

exit 0
"#;
        write_executable(&dir.join("rclone"), script);
    }

    /// Writes an executable POSIX-sh script at `dir/rclone` that logs its
    /// full argv and selected path list, then implements the exact-path
    /// `lsjson --files-from` query used by `file_sizes`.
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

files_from=""
while [ "$#" -gt 1 ]; do
  case "$1" in
    --files-from) files_from="$2"; shift 2 ;;
    --disable) shift 2 ;;
    *) shift ;;
  esac
done
target="$1"
root=$(strip_backend "$target")

if [ ! -d "$root" ]; then
  echo "directory not found: $root" >&2
  exit 3
fi

cp "$files_from" "$here/files.log"

first=1
printf '['
while IFS= read -r rel; do
  [ -n "$rel" ] || continue
  path="$root/$rel"
  [ -f "$path" ] || continue
  name=$(basename "$path")
  size=$(wc -c < "$path" | tr -d ' ')
  if [ "$first" = 1 ]; then
    first=0
  else
    printf ','
  fi
  printf '{"Name":"%s","Size":%s}' "$name" "$size"
done < "$files_from"
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

    #[test]
    fn file_sizes_returns_the_requested_object_size() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let hash = a_hash();
        let stdout = format!(r#"[{{"Name":"{hash}","Size":42}}]"#);
        fixed_response_rclone(fixture.path(), &stdout, "", 0);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert_eq!(
            remote
                .file_sizes(&[hash], &Cancel::new())
                .unwrap()
                .get(&hash),
            Some(&42)
        );
    }

    #[test]
    fn file_sizes_omits_a_requested_object_that_is_not_listed() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), "[]", "", 0);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert!(
            remote
                .file_sizes(&[a_hash()], &Cancel::new())
                .unwrap()
                .is_empty()
        );
    }

    /// An unreachable remote must surface as `Err`, never silently collapse to
    /// "the object is absent".
    #[test]
    fn file_sizes_returns_an_error_when_the_remote_cannot_be_reached() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        fixed_response_rclone(fixture.path(), "", "fatal error: invalid credentials", 7);
        let remote = remote_at(fixture.path(), temp_dir.path());

        let err = remote.file_sizes(&[a_hash()], &Cancel::new()).unwrap_err();
        assert!(matches!(
            err,
            RemoteError::Failed {
                exit_code: Some(7),
                ..
            }
        ));
    }

    #[test]
    fn file_sizes_queries_sixty_objects_with_one_scoped_rclone_call() {
        let fixture = tempfile::tempdir().unwrap();
        let remote_root = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        listing_rclone(fixture.path());

        let hashes = (0..60)
            .map(|byte| Sha256::from_digest([byte; 32]))
            .collect::<Vec<_>>();
        for (index, hash) in hashes.iter().enumerate() {
            let path = remote_root
                .path()
                .join(format!("files/sha256/{}/{}", hash.prefix(), hash));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![index as u8; index + 1]).unwrap();
        }

        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.path().display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let sizes = remote.file_sizes(&hashes, &Cancel::new()).unwrap();

        for (index, hash) in hashes.iter().enumerate() {
            assert_eq!(sizes.get(hash), Some(&((index + 1) as u64)));
        }

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        let remote_root_url = format!("local:{}", remote_root.path().display());
        assert_eq!(
            log.lines().count(),
            1,
            "metadata must use one process: {log}"
        );
        assert!(
            log.contains("lsjson --recursive --files-only --no-mimetype --no-modtime --disable ListR --files-from "),
            "argv: {log}"
        );
        assert!(log.trim_end().ends_with(&remote_root_url), "argv: {log}");
        let listed = std::fs::read_to_string(fixture.path().join("files.log")).unwrap();
        let expected = hashes
            .iter()
            .map(|hash| format!("files/sha256/{}/{}", hash.prefix(), hash))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(listed.trim_end(), expected);
    }

    #[test]
    fn retries_only_rclones_own_temporary_exit_class_and_gives_up_once_exhausted() {
        let fixture = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        // Fails (exit 5, "temporary") twice, then succeeds on the 3rd try --
        // exactly what retry_max's default of 3 total attempts allows.
        flaky_rclone(fixture.path(), 2, 5);
        let remote = remote_at(fixture.path(), temp_dir.path());

        assert!(
            remote
                .file_sizes(&[a_hash()], &Cancel::new())
                .unwrap()
                .is_empty()
        );
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
        // amount of retry_max should cause a second attempt.
        flaky_rclone(fixture.path(), 10, 7);
        let remote = remote_at(fixture.path(), temp_dir.path());

        let err = remote.file_sizes(&[a_hash()], &Cancel::new()).unwrap_err();
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
        let handle = std::thread::spawn(move || remote.file_sizes(&[a_hash()], &cancel_for_worker));

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
    fn copy_to_remote_skips_an_existing_same_size_remote_object() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let hash = a_hash();
        let rel = Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash));
        let files_dir = cache.path().join("files");
        std::fs::create_dir_all(files_dir.join(rel.parent().unwrap())).unwrap();
        std::fs::write(files_dir.join(&rel), b"object bytes").unwrap();
        // `remote_root` is the composed URL's target; `files_url()` appends
        // its own "/files", so the pre-existing object must live one level
        // deeper than `remote_root` itself.
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(remote_root.join("files").join(rel.parent().unwrap())).unwrap();
        // Already present on the "remote" -- must survive untouched.
        std::fs::write(remote_root.join("files").join(&rel), b"already good").unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let files_dir_utf8 = Utf8PathBuf::from_path_buf(files_dir.clone()).unwrap();
        remote
            .copy_to_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        assert_eq!(
            std::fs::read(remote_root.join("files").join(&rel)).unwrap(),
            b"already good",
            "--ignore-existing must prevent overwriting a good remote copy with local rot"
        );

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert!(
            !log.contains("\ncopy "),
            "same-size remote object should not upload: {log}"
        );
        assert!(
            !log.contains("\nmove "),
            "same-size remote object should not publish: {log}"
        );
    }

    #[test]
    fn copy_to_remote_stages_the_batch_then_publishes_it() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let hash = a_hash();
        let rel = Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash));
        let files_dir = cache.path().join("files");
        std::fs::create_dir_all(files_dir.join(rel.parent().unwrap())).unwrap();
        std::fs::write(files_dir.join(&rel), b"object bytes").unwrap();
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(&remote_root).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        let files_dir_utf8 = Utf8PathBuf::from_path_buf(files_dir.clone()).unwrap();
        remote
            .copy_to_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        assert_eq!(
            std::fs::read(remote_root.join("files").join(&rel)).unwrap(),
            b"object bytes"
        );
        assert!(
            !remote_root
                .join("tmp")
                .join(sanitize_stage_component(
                    &std::env::var("USER")
                        .or_else(|_| std::env::var("USERNAME"))
                        .unwrap_or_else(|_| "unknown".to_owned())
                ))
                .join("files")
                .join(&rel)
                .exists(),
            "published upload should be moved out of the staging namespace"
        );

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert!(log.contains("\ncopy --checksum --temp-dir "), "argv: {log}");
        assert!(
            log.contains("\nmove --ignore-existing --temp-dir "),
            "argv: {log}"
        );
        assert!(
            log.contains(temp_dir.path().to_str().unwrap()),
            "--temp-dir must point at this remote's own temp_dir, argv: {log}"
        );
        let stdout_modes = std::fs::read_to_string(fixture.path().join("stdout-mode.log")).unwrap();
        assert_eq!(stdout_modes, "pipe\npipe\npipe\npipe\npipe\n");
        let stderr_modes = std::fs::read_to_string(fixture.path().join("stderr-mode.log")).unwrap();
        assert_eq!(stderr_modes, "pipe\npipe\npipe\npipe\npipe\n");
    }

    #[test]
    fn copy_to_remote_uses_five_rclone_calls_for_sixty_objects() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let files_dir = cache.path().join("files");
        let rel_paths = (0..60)
            .map(|byte| {
                let hash = Sha256::from_digest([byte; 32]);
                let rel = Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash));
                let path = files_dir.join(&rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, vec![byte; usize::from(byte) + 1]).unwrap();
                rel
            })
            .collect::<Vec<_>>();
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(&remote_root).unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());

        remote
            .copy_to_remote(
                &Utf8PathBuf::from_path_buf(files_dir).unwrap(),
                &rel_paths,
                &Cancel::new(),
            )
            .unwrap();

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        let commands = log.lines().collect::<Vec<_>>();
        assert_eq!(
            commands.len(),
            5,
            "push must have fixed process count: {log}"
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("lsjson "))
                .count(),
            3,
            "push must batch each metadata phase: {log}"
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("copy "))
                .count(),
            1,
            "push must batch the upload: {log}"
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("move "))
                .count(),
            1,
            "push must batch publication: {log}"
        );
    }

    #[test]
    fn transfers_pass_rclone_progress_when_enabled() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let hash = a_hash();
        let rel = Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash));
        let files_dir = cache.path().join("files");
        std::fs::create_dir_all(files_dir.join(rel.parent().unwrap())).unwrap();
        std::fs::write(files_dir.join(&rel), b"object bytes").unwrap();
        let remote_root = cache.path().join("remote-root");
        std::fs::create_dir_all(&remote_root).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned())
        .with_transfer_progress(true);

        let files_dir_utf8 = Utf8PathBuf::from_path_buf(files_dir).unwrap();
        remote
            .copy_to_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();
        remote
            .copy_from_remote(&files_dir_utf8, std::slice::from_ref(&rel), &Cancel::new())
            .unwrap();

        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert!(log.contains("\ncopy --progress --checksum "), "argv: {log}");
        assert!(
            log.contains("\nmove --progress --ignore-existing "),
            "argv: {log}"
        );
        assert!(
            log.contains("\ncopy --progress --ignore-existing "),
            "argv: {log}"
        );
        let stdout_modes = std::fs::read_to_string(fixture.path().join("stdout-mode.log")).unwrap();
        assert_eq!(
            stdout_modes,
            "pipe\nterminal 24 80\npipe\nterminal 24 80\npipe\nterminal 24 80\n"
        );
        let stderr_modes = std::fs::read_to_string(fixture.path().join("stderr-mode.log")).unwrap();
        assert_eq!(stderr_modes, "pipe\npipe\npipe\npipe\npipe\npipe\n");
    }

    #[test]
    fn transfer_progress_capture_keeps_the_failure_tail() {
        let mut captured = PipeCapture::new(Capture::Tail(16));
        captured.extend(b"progress line before the final error");

        let message = captured.into_string();

        assert!(message.starts_with("[rclone output truncated]\n"));
        assert!(message.ends_with("he final error"));
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
    fn copy_from_remote_transfers_sixty_objects_with_one_rclone_call() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let remote_root = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        copying_rclone(fixture.path());

        let rel_paths = (0..60)
            .map(|byte| {
                let hash = Sha256::from_digest([byte; 32]);
                Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash))
            })
            .collect::<Vec<_>>();
        for (index, rel_path) in rel_paths.iter().enumerate() {
            let source = remote_root.path().join("files").join(rel_path);
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(source, vec![index as u8; index + 1]).unwrap();
        }

        let remote = RcloneRemote::new(
            format!("local:{}", remote_root.path().display()),
            Utf8PathBuf::from_path_buf(temp_dir.path().to_owned()).unwrap(),
        )
        .with_rclone_bin(fixture.path().join("rclone").to_str().unwrap().to_owned());
        let cache_files_dir = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        remote
            .copy_from_remote(&cache_files_dir, &rel_paths, &Cancel::new())
            .unwrap();

        for (index, rel_path) in rel_paths.iter().enumerate() {
            assert_eq!(
                std::fs::read(cache.path().join(rel_path)).unwrap(),
                vec![index as u8; index + 1]
            );
        }
        let log = std::fs::read_to_string(fixture.path().join("argv.log")).unwrap();
        assert_eq!(
            log.lines().count(),
            1,
            "transfer must use one process: {log}"
        );
        assert!(
            log.contains("copy --ignore-existing --temp-dir ") && log.contains(" --files-from "),
            "argv: {log}"
        );
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
        assert!(remote.file_sizes(&[], &Cancel::new()).unwrap().is_empty());
    }

    #[test]
    fn fake_remote_omits_an_absent_object_without_an_error() {
        let remote = FakeRemote::new();
        assert!(
            remote
                .file_sizes(&[a_hash()], &Cancel::new())
                .unwrap()
                .is_empty()
        );
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
    fn fake_remote_marking_unreachable_makes_every_operation_fail_rather_than_report_empty() {
        let remote = FakeRemote::new();
        remote.set_unreachable();
        let err = remote.file_sizes(&[a_hash()], &Cancel::new()).unwrap_err();
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
