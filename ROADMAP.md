# Roadmap

## Direction

`git-sfs` should stay as simple as possible: a thin layer on top of Git, the
filesystem, and `rclone`.

The source of truth should be visible files and symlinks. Avoid manifests,
databases, daemons, custom protocols, and hidden metadata.

## Done

- Go CLI scaffold with `cmd/git-sfs`
- Commands:
  - `git-sfs init`
  - `git-sfs setup`
  - `git-sfs add`
  - ~~`git-sfs status`~~ (deferred post-v1)
  - `git-sfs verify`
  - `git-sfs push`
  - `git-sfs pull`
  - ~~`git-sfs gc`~~ (deferred post-v1)
- Cache layout:
  - content files keyed by SHA-256
  - `tmp`
  - `locks`
- SHA-256 hashing and verification
- Git symlink validation
- `.git-sfs/cache` cache-root symlink
- Cache path resolution:
  - `--cache`
  - `GIT_SFS_CACHE`
  - `.git-sfs/cache`
- `.git-sfs/config.toml` parsing and validation
- Rejection of cache paths in `.git-sfs/config.toml`
- Filesystem remote backend
- Rclone command backend
- Local rclone integration tests
- Partial pull by file or directory
- Retry-safe local file writes with temp files and rename
- Cache locking for mutating operations
- Unit and integration tests
- Shell workflow suite
- GitHub CI
- GitHub release workflow
- Release archive build script
- Install script
- README
- AGENTS.md
- Justfile

## Partial

- `verify`
  - Detects key problems and prints stable category counts.
  - Optional remote checks and refined exit-code documentation still pending.
- rclone remote
  - Thin wrapper around the `rclone` CLI, not a custom cloud API.
  - Has local-backend integration coverage; cloud remotes still need validation.
- Concurrency
  - Cache-level lock exists.
  - Needs broader concurrent command tests.

## Remaining v1 Tasks

- Improve `git-sfs verify`
  - Verify all required invariants with precise messages.
  - Add optional remote existence verification.
  - Ensure corrupt remote files are rejected.
- Expand Git integration tests
  - One real workflow test already uses `git init`, `git add`, and `git clone`.
  - Confirm symlinks are tracked as symlinks.
  - Confirm `.git-sfs/` remains ignored.
- Keep partial pull behavior strong
  - `git-sfs pull <file>` must download only files needed for that path.
  - `git-sfs pull <directory>` must download only files needed below that directory.
  - Add coverage for mixed present/missing cache files.
- Add typed errors
  - Missing cache config
  - Invalid config
  - Invalid symlink
  - Missing cached file
  - Corrupt cached file
  - Missing remote file
  - Corrupt remote file
- Add fault-injection tests
  - Partial copy
  - Hash mismatch
  - Missing remote file
  - Corrupt remote file
  - Disk/write failure where practical
- Strengthen concurrency tests
  - Concurrent pull
  - Concurrent push
  - Concurrent add of duplicate content
- Improve remote publish safety
  - Confirm temp path cleanup behavior.
  - Confirm final file is never corrupt after interrupted upload.
- Review cross-platform path behavior
  - macOS
  - Linux
  - arm64
  - x86_64
- Dogfood a full workflow
  - Create repo
  - Add files
  - Push files
  - Clone elsewhere
  - Setup cache
  - Pull files
  - Verify files open normally
- Add cloud rclone integration tests
  - Gate behind environment variables.
  - Test upload, skip existing, pull, interruption retry, and permission errors.

## Production Readiness

- Declare and enforce minimum external tool versions
  - Add `min_rclone_version` field to `.git-sfs/config.toml` (optional, respected at runtime).
  - On each remote operation, parse `rclone version` output and fail with a clear message if below the declared minimum.
  - Ship a sensible default minimum version for rclone in the init template.
- Add `git-sfs version` command
  - Print the embedded git-sfs version.
  - Detect and print the rclone version found on `$PATH`.
- Add `--dry-run` flag to `push` and `pull`
  - Print what would be transferred without touching the remote or cache.
- Stabilize and document exit codes
  - Define a stable exit-code contract (0 = success, 1 = usage/config error, 2 = I/O or remote error, 3 = hash/integrity failure).
  - Cover exit codes in README for scripting use.
- Add config schema version field
  - Add `schema_version` to `config.toml` so future breaking changes can be detected and rejected with a migration hint rather than a silent misparse.
- Add shell completions
  - Bash, Zsh, and Fish completion scripts generated and shipped in the release archive.
- Platform smoke tests
  - Add CI matrix jobs that run the shell workflow suite on macOS and Linux (amd64 and arm64).

## Non-goals for v1

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
