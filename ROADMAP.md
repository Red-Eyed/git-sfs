# Roadmap

## Direction

`git-sfs` should stay as simple as possible: a thin layer on top of Git, the
filesystem, and `rclone`.

The source of truth should be visible files and symlinks. Avoid manifests,
databases, daemons, custom protocols, and hidden metadata.

## Shipped (v1.x)

- `verify` — presence and integrity checks, stable exit codes, optional remote checks, orphan detection
- `doctor` — 10-check diagnostic: repo, config, version requirements, cache, rclone, per-remote connectivity
- Pre-flight validation — rclone on PATH, config file existence, backend reachable, remote path exists — checked before every push/pull/verify
- Typed errors — missing config · invalid config · invalid symlink · missing/corrupt cache · missing/corrupt remote file
- Explicit error messages — "not a git repository", "config file not found (run git-sfs init)", "rclone config file not found", actionable hints throughout
- Retry with exponential backoff — configurable `retry_max`
- Disk space guard — estimate bytes needed before pull, fail early
- `min_rclone_version` and `min_git_sfs_version` — enforced at startup
- Submodule support — `.git` file recognized as repo root alongside `.git` directory
- Cache file immutability — all cache files written mode 0444; permission verified by `verify` without `--with-integrity`
- CHANGELOG — Keep a Changelog format, updated with each release

## Remaining

Importance: **H** high · **M** medium · **L** low. Effort: **S** small (hours) · **M** medium (days) · **L** large (week+).

| # | Task | Description | Importance | Effort |
|---|------|-------------|:----------:|:------:|
| 1 | Stable exit codes | 0 success · 1 config/usage · 2 I/O or remote · 3 integrity failure. Document for scripting. | H | S |
| 2 | Disaster recovery docs | Cache-loss recovery (re-pull) and remote corruption recovery (re-push after verify). `docs/recovery.md`. | H | S |
| 3 | Fault-injection tests | Partial copy · hash mismatch · missing/corrupt remote file · disk/write failure. | H | M |
| 4 | Remote publish safety | Confirm temp path cleanup and that final remote files are never corrupt after interrupted upload. | H | M |
| 5 | Platform CI matrix | Run shell workflow suite on macOS and Linux, amd64 and arm64, in CI. | H | M |
| 6 | Dogfood end-to-end | Full workflow: create repo · add · push · clone · setup · pull · verify files open normally. | M | S |
| 7 | Concurrency tests | Concurrent pull · push · add of duplicate content. | M | M |
| 8 | `--dry-run` for push/pull | Print what would be transferred without touching remote or cache. | M | M |
| 9 | Real rclone in tests | Replace fake rclone shell stubs with the real binary using a `local:` backend. Fake stubs require manual flag maintenance and mask integration bugs (e.g. `--size-only` vs `--ignore-existing` divergence). Fault-injection scenarios that need controlled failures can inject bad files directly into the temp remote directory instead of intercepting rclone. Requires rclone on CI PATH. | H | M |
| 10 | Cloud rclone integration tests | Gated on env vars. Cover upload · skip existing · pull · interruption retry · permission errors. | M | M |
| 11 | Fuzz testing | Fuzz `config.toml` parsing, symlink target parsing, hash string parsing. Short corpus run in CI. | M | M |
| 12 | Shell completions | Bash, Zsh, and Fish completions in the release archive. | L | M |

## Planned

- `git-sfs gc` — remove orphaned cache files not referenced by any tracked symlink (`--dry-run` and `--files`)
- `git-sfs status` — show tracked symlinks, missing cache files, and unpushed hashes at a glance

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
