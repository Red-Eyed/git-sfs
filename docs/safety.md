# Safety

`merk` is built around content hashes and retry-safe file operations.

## Hash Verification

Every cached file path includes the SHA-256 hash of its bytes:

```text
files/sha256/ab/<full_hash>
```

`merk` verifies bytes before accepting downloaded files or using cached files.

## Local Writes

Local writes use a temporary file in the destination directory and then rename
into place.

This avoids publishing partial files as final cache entries.

## Remote Writes

Remote writes should upload to a temporary remote path and then publish to the
final path.

If an upload is interrupted, rerun:

```sh
merk push
```

Existing valid remote files are skipped.

## Broken Symlinks

Check for broken or stale symlinks:

```sh
merk status
merk verify
```

Repair local materialization:

```sh
merk materialize
```

## Cache Corruption

If a cached file is corrupt, `merk verify` reports it.

If the remote still has a valid copy, remove the corrupt cached file and pull:

```sh
merk pull <path>
merk verify
```

## Git Safety

Git-tracked symlink targets must be relative and point into `.ds/worktree`.

This prevents absolute machine-local cache paths from being committed.

## What merk Does Not Protect

`merk` v1 does not provide:

- encryption
- access control
- team file locking
- permissions tracking
- timestamp tracking
- automatic Git hooks

Use filesystem permissions, SSH/rclone configuration, and normal Git review for
those concerns.
