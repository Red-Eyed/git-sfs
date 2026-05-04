# Changelog

## v1.2

### Fixed
- `rclone lsjson` output is now parsed from stdout only; rclone log/warning lines written to stderr no longer corrupt JSON parsing (fixes "invalid character '/' after top-level value" on Windows-style remote paths).

### Changed
- Push and pull both require the remote root directory to exist before transferring any files. A missing root path now fails immediately with a clear message instead of silently creating files at the wrong location.
- `Ping` removed from the `Remote` interface — `RequireExists` covers both reachability and path-existence in one check.

### Added
- Fault-injection test coverage: hash-mismatch rejection on pull, no leftover temp files after interrupted download, `ErrMissingCachedFile` on push, no corrupt final file after interrupted upload, `Add` error when cache directory is read-only.

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
