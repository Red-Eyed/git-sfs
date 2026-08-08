# Architecture

This document is for contributors. For the user-facing model see
[Concepts](concepts.md).

## Workspace Layout

```text
crates/git-sfs/       CLI shell: argv, terminal output, signals, exit codes
crates/git-sfs-core/  reusable command logic and side-effect ports
scripts/              installer, release packaging, docs generation
test/workflows/       end-to-end workflows through the installer path
test/differential/    contract and compatibility harnesses
```

`git-sfs-core` cannot print and cannot exit. It depends on small domain values
and ports; the binary crate owns rendering, progress, signal handling, and exit
code mapping.

## Core Modules

```text
domain/    validated values: hashes, symlink targets, remote URLs, config
exec/      command implementations: add, import, mv, pull, push, verify, ...
plan/      pure planning for byte movement and disk-space decisions
ports/     filesystem, Git tree scanning, cache store, locks, rclone remote
cancel.rs  shared cancellation flag polled by byte-moving loops
error.rs   typed error categories surfaced to the CLI
```

The dependency direction is:

```text
git-sfs CLI -> git-sfs-core::exec -> domain + plan + ports
```

## Core Data Flow

### add

```text
repo.scan(paths)
  │  collect regular files Git does not already track
  ▼
per file
  ├─ hash stream          SHA-256, no full load into memory
  ├─ store.store_file     temp file -> verify -> readonly -> rename
  └─ publish symlink      relative target committed to Git
```

### push

```text
repo.scan(scope)          collect git-sfs symlinks
plan::push                deduplicate by hash; reject missing cache objects
remote.file_sizes         batch-list requested remote paths
remote.copy_to_remote     rclone copy --files-from to remote staging path
remote.copy_to_remote     verify staged sizes, then rclone move --files-from
```

Push stages under the configured remote itself, not a system temp directory.
The staging path includes a repository-specific and process-specific component
so overlapping pushes do not share one global temp path. Final remote objects
are published only after the batch transfer and basic size verification pass.

### pull

```text
repo.scan(scope)          collect git-sfs symlinks
plan::pull                deduplicate by hash; skip verified local objects
disk-space check          require enough free bytes for missing objects
remote.copy_from_remote   rclone copy --files-from into cache-local staging
store.adopt               hash-verify, set readonly mode, atomically publish
```

Pull downloads into `<cache>/tmp`, verifies bytes by hash, and only then makes
objects visible in `<cache>/files/sha256/...`.

## Symlink Format

Git-tracked symlinks use a relative target that threads through the repo-local
`.git-sfs/cache` indirection:

```text
<file> -> ../../.git-sfs/cache/files/sha256/<prefix>/<hash>
                              |
                              +-> symlink to machine-local cache root
```

This keeps absolute machine-local paths out of Git. `domain::symlink` enforces
the format: relative target, correct prefix match, valid lowercase SHA-256 hash.

## Cache Layout

```text
<cache>/
  files/sha256/<2-char-prefix>/<64-char-sha256-hash>   read-only after write
  tmp/                                                  staging for atomic ops
  locks/                                                directory-based locks
```

Cache objects are immutable once published. A writable legacy object is treated
as unverified: commands hash-check it before trusting it, then restore the
read-only mode.

## Remote Interface

`ports::remote` is the boundary around rclone. The high-level commands depend on
operations such as batch size listing, batch copy, and remote integrity checks;
rclone-specific argv construction stays inside the adapter.

Push and pull use batched `rclone copy --files-from` / `rclone move
--files-from` calls. Remote metadata checks list only requested prefixes where
possible. Per-object subprocesses are avoided unless the operation is inherently
object-specific, such as verifying remote bytes by downloading and hashing one
object.

## What Deliberately Does Not Exist

- No manifest file or database: the Git tree is the file list.
- No background service: every operation is a one-shot CLI invocation.
- No custom protocol: remotes use rclone.
- No distributed lock: the directory lock is single-machine only.
