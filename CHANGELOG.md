# Changelog

## v1.11.1

### Fixed
- `git-sfs status` detail lines no longer read ambiguously. The local and remote states are now written as explicit `local=` / `remote=` key-value pairs, so a file that is absent locally but present remotely shows `local=missing remote=present` (with its size recovered from remote metadata) instead of `missing on-remote`, which could be misread as "missing on the remote".

---

## v1.11.0

### Added
- `git-sfs status` reports each tracked file's size and whether it is cached locally — **without downloading any bytes**. By default it is local-only and makes no network calls. Pass `--remote NAME` to additionally check presence and recover sizes for not-yet-pulled files from that remote's metadata (`rclone lsjson`), so you can learn a file's size before pulling it. `--json` emits a machine-readable summary plus one record per symlink, and a path argument scopes the report to a subtree. Counts are aggregated over unique file contents, and `status` is informational — it always exits `0`.
- `git-sfs remotes` lists the remotes configured in `.git-sfs/config.toml` (name, backend, path, rclone config), marking the `default` remote, with a `--json` form. It reads only the committed config and never contacts a backend.

---

## v1.10.0

### Added
- Long-running operations (`add`, `import`, `verify`, `setup`, `pull`) are cancelable with `Ctrl-C`. The hash, copy, and download loops check for cancellation on every chunk, so an interrupt stops promptly instead of running to completion. Cancellation is safe — an interrupted write never publishes a partial cache file (temp + rename) — and an interrupted run reports `canceled` and exits with status `130`.
- `add` and `import` show byte-weighted hashing progress: the bar advances by bytes hashed, giving smooth within-file progress for large files instead of a count that jumps straight from 0 to 100%.

### Changed
- Local hashing progress is now live *during* the hash phase instead of only after it. On a terminal the bar redraws in place; when output is redirected to a log or CI, it emits periodic percentage lines (bounded to ~100) so long jobs stay visibly live.

### Fixed
- `git-sfs add` and `git-sfs import` no longer appear frozen for the entire hashing phase. Progress was previously emitted only in the post-hash symlink step, so a multi-hour add showed nothing until it finished and then dumped all output at once.

---

## v1.9.0

### Added
- `-j`/`--jobs` global flag sets the parallel worker count from the command line, overriding `[settings].n_jobs`. The `App.Jobs` capability already existed but no flag exposed it.
- Per-command help: `git-sfs <command> --help` lists that command's flags and arguments.

### Changed
- Command-line parsing now uses `github.com/alecthomas/kong`. Global flags (`--cache`, `--config`, `--verbose`, `--quiet`, `-j`, `--version`) may now appear before or after the command name; previously a flag placed after the subcommand was silently ignored.
- `git-sfs init --force` is a validated flag instead of a hand-scanned argument.
- Local hashing progress (`add`/`import`/`setup`) redraws in place on a terminal and prints a single summary line on non-terminals (CI, pipes) instead of one line per file.

### Fixed
- `push`/`pull` now show rclone's transfer progress whenever output is not silenced with `--quiet`. Previously the bar appeared only in `--verbose` mode, so normal transfers showed no progress at all.

---

## v1.8.5

### Fixed
- `pull` no longer re-hashes already-protected cache files. `Protect` now checks the file's write-protection bit before hashing: a read-only file was written by a prior `Protect` call that already verified it, so the bytes cannot have changed. Only newly-downloaded files (which rclone writes with normal permissions) go through SHA-256 verification.

---

## v1.8.4

### Added
- `git-sfs init` now writes `.git-sfs/README.md` — a brief orientation for anyone who opens that directory, listing all paths it contains and linking to the full documentation. The file is skipped if it already exists.

---

## v1.8.3

### Fixed
- Upgrade URL in version-check error message corrected to `https://github.com/Red-Eyed/git-sfs`; URL is now stored in `config.RepoURL` constant.

---

## v1.8.2

### Fixed
- `pull` no longer re-hashes files already in the cache. `HasValid` now checks write-protection via `stat` instead of reading and SHA-256ing the full file — a read-only file at the content-addressed path is sufficient proof of integrity. Files written by older versions (which lacked write protection) are hash-verified once on first access and protected in place.

---

## v1.8.1

### Fixed
- `--config /absolute/path` no longer has the repo root prepended to it. `filepath.Join` strips the leading slash from an absolute second argument instead of treating it as rooted, so absolute config paths were silently resolved to `<repo>/absolute/path`.

---

## v1.8.0

### Added
- Cache files are now stored and enforced as read-only (mode `0444`). Any cache file found with write permission is rejected at read time, protecting against accidental mutation of immutable content-addressed data.

### Fixed
- `push` uses `--size-only` comparison to prevent silent corruption when a previous upload was interrupted mid-transfer and left a truncated file on the remote.

---

## v1.7.0

### Added
- `git-sfs doctor` — new diagnostic command that runs ten sequential checks and reports pass/fail for each: git repository, config file, git-sfs version, cache config, cache writability, rclone binary, rclone version, and per-remote checks (rclone config file, backend reachable, remote path exists). Exits non-zero if any check fails. Useful for verifying a new machine setup before running a workflow.
- `min_git_sfs_version` setting in `[settings]` — fail fast if the installed git-sfs is older than the declared minimum. Enforced at the start of every command.
- `min_rclone_version` setting in `[settings]` — previously parsed but never enforced; now verified before any remote operation.

### Fixed
- Submodule support: `git-sfs` now recognises a `.git` file (used by Git submodules) as a repository root. Previously it required a `.git` directory and would walk up to the parent repository, rooting all paths to the wrong tree.
- Explicit error messages throughout: "not a git repository", "config file not found: … (run git-sfs init)", "missing cache config: set GIT_SFS_CACHE, pass --cache, or run git-sfs setup", "rclone config file not found: …", and "cannot connect to remote … (check rclone config)" — instead of raw OS errors with no context.
- Remote connectivity check (`lsd`) no longer silently swallows rclone config errors (e.g. "didn't find section in config file") that previously caused a misleading "remote path does not exist" message.
- `push` now goes through the same `preflight` path as `pull` and `verify`, ensuring consistent rclone-on-PATH and version checks.

---

## v1.6.3

### Fixed
- `git-sfs mv` now supports moving directories of symlinks, not just individual files. All git-sfs symlinks under the source directory are relocated recursively; empty source directories are cleaned up.
- `git-sfs mv <src> <new/deep/path>` now creates intermediate parent directories automatically (both for single-symlink and directory moves).
- `git-sfs mv` works correctly on broken symlinks (files tracked but not yet in local cache).

---

## v1.6.2

### Fixed
- `--verbose` progress bar now renders correctly. Previously rclone received an `io.Writer` pipe instead of the real file descriptor, so it could not detect a TTY and suppressed the animated `--progress` display.

---

## v1.6.1

### Fixed
- `install.sh` is now attached to each GitHub release as an asset, enabling installation via `https://github.com/Red-Eyed/git-sfs/releases/latest/download/install.sh` for environments where `raw.githubusercontent.com` is blocked by a corporate proxy.

---

## v1.6.0

### Added
- `git-sfs mv <src> <dst>` — relocates a git-sfs symlink and rewrites its relative target for the new directory depth. Use this instead of `git mv` when moving files across directory levels.
- `--verbose` now streams rclone's live progress output (`--progress`) directly to stderr during push and pull, so transfer speed and ETA are visible.
- Lock contention is now reported: if a command blocks waiting for another git-sfs process to finish, it prints `waiting for lock <name> (held by pid: …)` immediately instead of hanging silently.

### Changed
- `import` copies source files by default, leaving the source intact. Pass `--move` to consume the source (rename on the same filesystem, copy-verify-remove across filesystems).

---

## v1.5.2

### Changed
- `import` now copies source files by default, leaving the source intact. Pass `--move` to consume the source (rename on the same filesystem, copy-verify-remove across filesystems). Source directories and symlinks are only removed when `--move` is given.

---

## v1.5.1

### Fixed
- `import` no longer attempts to `chmod` the source file before moving it into cache. The chmod was redundant (the staging file is made read-only before the final rename) and caused "permission denied" when importing from mounts owned by another user.

---

## v1.5

### Changed
- Push and pull now issue a single `rclone copy --ignore-existing --files-from <list>` call instead of one subprocess per file. All files are transferred in one rclone invocation; rclone's internal `--transfers` parallelism handles concurrency. This eliminates per-file connection overhead and reduces the number of rclone processes from O(N) to one.

---

## v1.4

### Fixed
- Push no longer overwrites an existing remote file. After uploading to a temp path, git-sfs re-checks whether the final destination already exists before issuing the rename. If another push landed the same file concurrently, the temp upload is discarded. Remote files are content-addressed and immutable — once written they are never touched again.

---

## v1.3

### Added
- Backend connectivity check before every push and pull: git-sfs now probes the backend root (e.g. `smb:`, `sftp:`, `s3:bucket`) with a lightweight call before checking the configured path. A broken rclone config or unreachable network produces a clear `"cannot connect to remote (check rclone config)"` error instead of a misleading `"path does not exist"` message.

---

## v1.2

### Added
- `min_rclone_version` setting in `[settings]`: if set, git-sfs detects the installed rclone version and refuses to run if it is below the required minimum (e.g. `min_rclone_version = "1.67.0"`).
- `retry_max` setting in `[settings]`: configures how many times a failed rclone call is retried with exponential backoff (default 3).
- Push and pull check that rclone is on `PATH` before attempting any transfer.
- Push and pull verify that the remote root directory exists before transferring any files; a missing root now fails immediately with a clear message instead of silently creating files at a wrong path.
- Disk-space guard: before pull, git-sfs sums the byte sizes of missing remote files and fails early if the cache volume has less than 110% of the required space available.
- Exit codes are now stable: 0 = success, 1 = config/usage error, 2 = I/O or remote error, 3 = integrity failure.
- `verify` reports orphaned cache objects (files in cache with no tracked symlink) as an informational hint.
- `verify --with-integrity` checks that cache file permissions are read-only (`0444`) in addition to verifying content hashes.

### Fixed
- `rclone lsjson` output is parsed from stdout only; rclone log/warning lines written to stderr no longer corrupt JSON parsing (fixes "invalid character '/' after top-level value" on remotes with Windows-style paths).
- Cache files are now set read-only immediately after being written to cache (`Cache.Move`); previously the permission was applied only on explicit protect calls.

### Changed
- Push and pull both require the remote root directory to exist. Previously pull only checked basic reachability and accepted a missing root path.

---

## v1.1

### Changed
- Remote backend config field renamed from `remote` to `backend`; `type` field removed (rclone handles backend detection).
- Filesystem (local copy) backend removed — use `rclone` with `backend = local` for local-path remotes.
- `-r` / `--remote` flag added to `push`, `pull`, and `verify` for selecting a named remote at the command line.

---

## v1.0

### Changed
- `gc` command removed; cache cleanup is deferred to a future release.
- `status` command removed; use `verify` instead.
- `Materialize` / `Dematerialize` internal helpers removed.

### Added
- SIGINT / SIGTERM handling for clean shutdown during long transfers.
- Parallel setup, materialize, and pull protect/link phases.
- Architecture documentation for contributors.

### Fixed
- `rclone lsjson` used for remote file existence checks (replaces `copyto` probe).

---

## v0.18

### Changed
- `verify` simplified; redundant status reporting removed.

---

## v0.17 – v0.14

### Added
- Configurable parallel jobs (`-j` / `--jobs` flag and `n_jobs` in `[settings]`).
- Simple progress output during push and pull.
- `--verbose` flag for command tracing (debug output).
- `--version` flag; version embedded into built binaries via `-ldflags`.

### Changed
- `mv` command renamed to `import`.
- Zero-copy move import: source file is moved into cache, not copied.
- Cross-filesystem import support.
- `import --follow` flag for symlink resolution.

---

## v0.13 – v0.9

### Added
- `rclone` remote backend (`backend = rclone` / direct rclone target config).
- `host` and `path` remote config fields for SSH/rclone targets.
- Installer respects `CA_BUNDLE` / `CURL_CA_BUNDLE` env vars.
- Typed config and symlink error sentinels.
- Git workflow integration tests.
- Shell workflow test suite (`test/workflows/run.sh`).

### Changed
- Project renamed from its prior name to `git-sfs`.
- Config format moved to `.git-sfs/config.toml` with commented starter template.
- Cache files set read-only (`0444`) after being written.

---

## v0.8 – v0.1

Initial development: content-addressed local cache, symlink tracking, `add`, `push`, `pull`, `verify`, and `gc` commands; GitHub Actions CI and release automation.
