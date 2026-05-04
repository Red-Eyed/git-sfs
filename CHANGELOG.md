# Changelog

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
