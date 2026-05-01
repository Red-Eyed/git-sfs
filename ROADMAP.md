# Roadmap

## Direction

`git-sfs` should stay as simple as possible: a thin layer on top of Git, the
filesystem, and `rclone`.

The source of truth should be visible files and symlinks. Avoid manifests,
databases, daemons, custom protocols, and hidden metadata.

## In Progress

- `verify` — detects key problems and prints stable counts; optional remote checks and exit-code docs still pending
- rclone remote — local-backend integration covered; cloud remotes still need validation
- Concurrency — cache-level lock exists; broader concurrent command tests still needed

## Remaining

Importance: **H** high · **M** medium · **L** low. Effort: **S** small (hours) · **M** medium (days) · **L** large (week+).

| # | Task | Description | Importance | Effort |
|---|------|-------------|:----------:|:------:|
| 1 | Cache file immutability | Write cache files mode 0444; verify mode in `verify`. Prevents silent data corruption. | H | S |
| 2 | Enforce minimum rclone version | `min_rclone_version` in `config.toml`; parse `rclone version` before remote ops and fail fast. | H | S |
| 3 | Stable exit codes | 0 success · 1 config/usage · 2 I/O or remote · 3 integrity failure. Document in README for scripting. | H | S |
| 4 | Disaster recovery docs | Cache-loss recovery (re-pull) and remote corruption recovery (re-push after verify). README or `docs/recovery.md`. | H | S |
| 5 | Expand Git integration tests | Confirm symlinks are tracked as symlinks and `.git-sfs/` stays ignored. | H | S |
| 6 | Improve `verify` | All invariants with precise messages, optional remote existence check, corrupt remote file rejection. | H | M |
| 7 | Typed errors | Missing config · invalid config · invalid symlink · missing/corrupt cache file · missing/corrupt remote file. | H | M |
| 8 | Pre-flight validation | Before push/pull: validate config, confirm rclone on PATH, confirm remote reachable. Fail fast, not mid-transfer. | H | M |
| 9 | Fault-injection tests | Partial copy · hash mismatch · missing/corrupt remote file · disk/write failure. | H | M |
| 10 | Disk space guard | Estimate bytes needed before pull; fail early if cache volume has insufficient free space. | H | M |
| 11 | Retry with backoff | Configurable retry count and exponential backoff around rclone calls; log each attempt. | H | M |
| 12 | Remote publish safety | Confirm temp path cleanup and that final remote files are never corrupt after interrupted upload. | H | M |
| 13 | Cross-platform path behavior | Verify correct behavior on macOS, Linux, arm64, x86_64. | H | M |
| 14 | Platform CI matrix | Run shell workflow suite on macOS and Linux, amd64 and arm64, in CI. | H | M |
| 15 | Config schema version | `schema_version` in `config.toml`; reject unknown versions with a migration hint. | M | S |
| 16 | `git-sfs version` | Print embedded version and detected rclone version. Useful for bug reports. | M | S |
| 17 | Partial pull coverage | Tests for mixed present/missing cache files. | M | S |
| 18 | CHANGELOG | User-facing changelog (Keep a Changelog format) updated with each release. | M | S |
| 19 | Dogfood end-to-end | Full workflow: create repo · add · push · clone · setup · pull · verify files open normally. | M | S |
| 20 | Orphan detection in `verify` | Report cache files not referenced by any tracked symlink. Prerequisite for `gc`. | M | M |
| 21 | Concurrency tests | Concurrent pull · push · add of duplicate content. | M | M |
| 22 | `--dry-run` for push/pull | Print what would be transferred without touching remote or cache. | M | M |
| 23 | Cloud rclone integration tests | Gated on env vars. Cover upload · skip existing · pull · interruption retry · permission errors. | M | M |
| 24 | Fuzz testing | Fuzz `config.toml` parsing, symlink target parsing, hash string parsing. Short corpus run in CI. | M | M |
| 25 | Shell completions | Bash, Zsh, and Fish completions in the release archive. | L | M |

## Post-v1

- `git-sfs status`
- `git-sfs gc`

## Non-goals

- Manifest files
- Tree files
- Git LFS server
- git-annex branch
- Custom Git protocol
- Database backend
- Background daemon
- Custom cloud API clients
- Web UI
- Encryption
- Compression
- Chunking
- Automatic Git hooks
