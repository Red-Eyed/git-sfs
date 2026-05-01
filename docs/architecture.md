# Architecture

This document is for contributors. For the user-facing model see [Concepts](concepts.md).

## Package Layout

```text
cmd/git-sfs/       entry point — parses args, calls cli.Run, exits on error
internal/
  cli/             flag parsing and command routing; no business logic
  core/            command implementations (App struct)
  config/          TOML parser (no third-party lib); strict unknown-field rejection
  cache/           content-addressed local store; atomic writes; read-only protection
  hash/            SHA-256 streaming; hex encoding; path prefix helpers
  fsutil/          atomic copy/rename; symlink creation; read-only chmod
  sfspath/         symlink target format: construction, parsing, validation
  materialize/     .git-sfs/cache hard-link management (cache ↔ repo binding)
  localstate/      repo root detection; cache symlink resolution and binding
  lock/            directory-based process lock with 100ms poll
  progress/        stderr progress bar driven by a goroutine
  remote/          Remote interface; rclone and filesystem backends
  errs/            sentinel errors
  version/         version string embedded at build time
```

The dependency direction is: `cli` → `core` → everything else. No package outside `core` imports `core`.

## Core Data Flow

### add

```text
filepath.WalkDir(paths)
  │  collect regular files
  ▼
parallel worker pool
  ├─ hash.File(path)          stream SHA-256, no full load into memory
  └─ cache.Store(path, hash)  atomic copy: temp file → rename

sequential (per file)
  ├─ materialize.Link         bind .git-sfs/cache/<hash> → cache/<hash>
  ├─ os.Remove(original)
  └─ os.Symlink(target, path) relative symlink committed to Git
```

### push

```text
collectGitSFSSymlinks(repo)   WalkDir; parse hash from each symlink in one pass
uniqueHashesFromTracked       deduplicate by hash

parallel worker pool (per unique hash)
  ├─ cache.HasValid(hash)     os.Stat check
  ├─ remote.HasFile(hash)     rclone lsjson — skip if already present
  └─ remote.PushFile(hash)    verify local bytes → rclone copyto tmp → rclone moveto dst → lsjson confirm
```

### pull

```text
collectGitSFSSymlinks(repo, path)
uniqueHashesFromTracked

parallel worker pool (per unique hash)
  └─ remote.PullFile(hash)    rclone copyto → verify hash → chmod read-only → rename

sequential (per hash)
  ├─ cache.Protect            make cache file read-only
  └─ materialize.Link         bind .git-sfs/cache/<hash> → cache/<hash>
```

## Worker Pool

All parallel work goes through `runIndexed` in `core/app.go`:

```text
jobs chan int  (unbuffered)
│
├─ enqueuer goroutine  sends indices; stops on ctx cancel
└─ N worker goroutines consume indices; first error cancels context via sync.Once
```

Worker count is capped at `min(configured_jobs, GOMAXPROCS, 4)` unless overridden with `-j`.

## Symlink Format

Git-tracked symlinks use a relative target that threads through the repo-local `.git-sfs/cache` indirection:

```text
<file> → ../../.git-sfs/cache/files/sha256/<prefix>/<hash>
                              │
                              └─ symlink → <machine-local cache root>/files/sha256/<prefix>/<hash>
```

This keeps absolute machine-local paths out of Git. `sfspath.ParseGitSymlink` enforces the format: relative target, correct prefix match, valid hex hash.

## Cache Layout

```text
<cache>/
  files/sha256/<2-char-prefix>/<64-char-sha256-hash>   read-only after write
  tmp/                                                  staging for atomic ops
  locks/                                                directory-based locks
```

Files are written via temp-file + rename (`fsutil.AtomicCopy`). After a pull or protect call, `os.Chmod` removes write bits so cache files are not accidentally modified.

## Remote Interface

```go
type Remote interface {
    HasFile(ctx, hash)            bool   // lsjson — cheap existence check
    CheckFile(ctx, hash)          bool   // download + hash verify — use only for integrity checks
    PushFile(ctx, hash, srcPath)  error  // verify local → upload to tmp → publish
    PullFile(ctx, hash, dstPath)  error  // download to tmp → verify → rename
}
```

Two backends: `rcloneRemote` (subprocess per call) and `filesystemRemote` (direct `os` calls). The rclone backend issues one subprocess per operation; there is no persistent rclone process.

## What Deliberately Does Not Exist

- No manifest file or database — the Git tree is the file list.
- No background service — every operation is a one-shot CLI invocation.
- No custom protocol — remotes use rclone, rsync, or the local filesystem.
- No distributed lock — the directory lock is single-machine only.
