Audit the git-sfs codebase for production-risk issues. Read every relevant Go source file under `internal/` and `cmd/` and report concrete findings only — no speculative concerns.

IMPORTANT CONSTRAINT: This project manages terabytes of data, millions of files. Re-hashing every file on every operation is not acceptable. When evaluating verification strategies, the goal is not "hash everything always" — it is "hash at the right moments with the right granularity." Flag cases where expensive hashing is redundant or missing where it actually matters, and flag cases where cheap signals (file size, permissions, mtime) are trusted when they shouldn't be.

Check each area below. For every finding, include: the file path and line number(s), a one-sentence description of the risk, and a suggested fix direction.

---

## 1. Data integrity — hash verification at write boundaries

Only write-time verification is required. Do NOT flag the absence of re-hashing on every read.

- After hashing a local file: is the computed hash validated before it is used as a cache key?
- After copying into cache: is the written file re-read and verified against the expected hash?
- After downloading from a remote: is the downloaded content verified before being published? Check specifically whether `Protect()` always re-hashes newly downloaded files or can skip verification when the file already has no write bits (e.g., if rclone or a backend delivers a file read-only).
- Is there any path where a file is accepted into cache or reported as present without any verification?

## 2. Atomic writes — temp-file + rename discipline

- Every write to a cache file or config file must go through a temp file and an atomic rename.
- Flag any `os.WriteFile`, `os.Create`, or direct `io.Copy` to a final path that skips the temp-rename pattern.
- Flag any temp file that is renamed before the write (or hash check) completes.

## 3. Context propagation and cancellation

- Every byte-moving loop (hash, copy, download, upload) must accept a `context.Context` and check it each chunk.
- Flag any loop that ignores the context, or any goroutine that does not respect cancellation.
- Flag any operation that can hang indefinitely without a deadline or cancellation path.

## 4. Error handling — silent drops and ignored errors

- Flag any `_ = err` or blank assignment where the error represents a data-loss or corruption risk.
- Flag any error that is logged but not propagated when it should abort the operation.
- Flag any defer that discards a close error on a file being written (as opposed to read-only).

## 5. Cache immutability — write-once enforcement

- Cache files are supposed to be written once and then treated as read-only.
- Flag any path that overwrites or truncates an existing cache file.
- Flag any chmod or permission change applied to files the user did not explicitly hand to git-sfs.

## 6. Symlink correctness and target validation

- Symlinks must point into `.git-sfs/cache/files/sha256/<2-char prefix>/<full hash>` using a relative path.
- Flag any code that constructs a symlink target with an absolute path.
- Flag any code that does not validate the symlink target format before creating or following a link.
- Check `materialize.Link()`: does it verify that the symlink target actually points to the correct hash path, or only that the symlink file exists? A symlink pointing to the wrong hash (e.g., after a failed migration) would pass an existence check but serve wrong data.

## 7. Concurrency safety

- Flag any shared mutable state accessed from multiple goroutines without a lock or atomic.
- Flag any channel send or receive that can block forever (no select with a done/ctx case).
- Flag any goroutine leak: goroutines that are started but have no termination path when an error occurs or the context is cancelled.

## 8. Resource leaks

- Flag any `os.Open` / `os.OpenFile` / `os.Create` without a paired `defer f.Close()` in all exit paths.
- Flag any HTTP response body that is not closed.
- Flag any subprocess (exec.Cmd) whose stdout/stderr pipes are opened but not fully drained before `Wait()`.

## 9. Input validation at boundaries

- Config fields read from `config.toml`: are remote names and cache paths validated before use?
- File paths from the user (CLI args): are they validated to stay within expected directories (no path traversal)?
- Hash strings from file names or remote listings: are they validated as well-formed SHA-256 hex before use?

## 10. Partial-state and crash-recovery safety

- If the process is killed mid-operation, what on-disk state is left? Is it always safe to re-run the same command?
- Flag any sequence where a visible side effect (symlink creation, config write) happens before the data it depends on is durably written.
- Flag lock file cleanup on crash: the lock uses directory creation (`os.Mkdir`). If the process is killed while holding the lock, the directory is not removed. Check whether there is any stale-lock detection (e.g., checking if the owner PID is still alive). If not, a single crash permanently blocks all subsequent operations until the user manually removes the lock.

## 11. Bit rot — long-term cache file integrity without re-hashing

This is a year-scale risk. Cache files are hash-verified at write time, then trusted by their read-only permission bit forever. Re-hashing every file on every access is not feasible at terabyte scale.

Check whether the codebase has any strategy for detecting silent corruption of already-cached files. Look for:
- A `doctor` or `verify` command that does on-demand hash verification without being on the hot path.
- Any incremental or sampling approach (e.g., verify N files per run, or verify files not touched in >N days).
- Any use of filesystem-native checksums (e.g., macOS APFS or ZFS file integrity) or extended attributes to record a verification timestamp.

If none of these exist, flag it: there is currently no mechanism to detect bit rot or silent storage corruption in existing cached files, and a user with terabytes of data has no supported way to audit cache integrity without re-pulling everything.

## 12. Remote integrity — push `--size-only` and corrupt-but-same-size remote files

`CopyToRemote` uses rclone's `--size-only` flag. This means a remote file that is corrupt but has the same byte count as the correct file is never re-uploaded — it silently stays corrupt on the remote forever.

Re-downloading every remote file to verify its hash is not feasible at terabyte scale. Instead, check:
- Does rclone's `--checksum` flag work for the configured backends? `--checksum` uses the remote's own checksum (if supported) instead of a full download — much cheaper than re-downloading.
- Is there any `verify-remote` or `fsck`-style command that can detect this corruption on demand (not on the hot path)?
- If a user knows a remote file is corrupt, is there a supported way to force re-upload of a specific file?

If none of these exist, flag it: same-size remote corruption is undetectable without a full pull-and-verify, and there is no command to force re-push of an individual file.

## 13. Disk space preflight — silent size omission on transient errors

In `checkDiskSpace`, when `r.FileSize` returns an error (transient network failure, rclone error), the error is recorded but the file's size contribution is simply excluded from the total (`if size > 0 { total.Add(size) }`). Check:
- Is a transient `FileSize` error silently treated as "file not found" rather than propagated?
- Can a run of transient errors produce a false-OK on the disk space check, allowing a pull to start that will exhaust disk space mid-download?

## 14. rclone `--temp-dir` coverage — cross-filesystem atomic rename

`CopyFromRemote` passes `--temp-dir` to rclone only when `r.tempDir != ""`. Without it, rclone uses the OS default temp dir (`/tmp`), which may be on a different filesystem from the cache. Rclone's internal rename would then cross filesystems, which is either an error or falls back to a non-atomic copy+delete.

Check all code paths that construct a `rcloneRemote` (via `NewRclone`, `NewRcloneTarget`, `NewRcloneTargetWithOptions`, `NewWithOptions`) and verify that `TempDir` is always populated in every production code path. If there are paths where `TempDir` is empty, flag each call site.

---

After scanning, output:

**FINDINGS** — one entry per issue:
```
[AREA] file:line — risk description — suggested fix
```

**CLEAN** — list areas where you found no issues.

**SUMMARY** — total finding count and the top 1–3 highest-risk items that could cause data loss or corruption in production.

**CHECKLIST** — a markdown task list of every finding, one line each, ordered by severity (highest first). Format each line so it can be pasted directly into a tracking document or GitHub issue:

```
- [ ] [AREA] `file:line` — one-sentence description
```

High-severity items (data loss / silent corruption) first, then medium (operational risk), then low (documentation / observability gaps).
