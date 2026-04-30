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
  - `git-sfs status`
  - `git-sfs verify`
  - `git-sfs push`
  - `git-sfs pull`
  - `git-sfs gc`
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

- `status` and `verify`
  - Current implementation detects key problems and prints stable category counts.
  - Still needs optional remote checks and refined exit-code documentation.
- rclone remote
  - Thin wrapper around the `rclone` CLI, not a custom cloud API.
  - Has local-backend integration coverage; cloud remotes still need validation.
- Concurrency
  - Cache-level lock exists.
  - Needs broader concurrent command tests.
- Garbage collection
  - Protects currently referenced files.
  - Needs more careful dry-run output and safety tests.
- Error handling
  - Errors are clear enough for early use.
  - Needs typed errors for important failure classes.

## Remaining v1 Tasks

- Improve `git-sfs status`
  - Separate tracked symlinks, missing cached files, corrupt cached files,
    missing remote files, broken Git symlinks, and unconverted files.
  - Make output stable enough for CI parsing.
  - Define exit codes.
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
- Improve `gc`
  - Better dry-run report.
  - Cached-file cleanup only; no per-file local link layer.
  - Tests for keeping all referenced files.
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
