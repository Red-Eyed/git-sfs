# Safety

`git-sfs` is built around content hashes and retry-safe file operations.

## Hash Verification

Every cached file path includes the SHA-256 hash of its bytes:

```text
files/sha256/ab/<full_hash>
```

`git-sfs` verifies bytes before accepting downloaded files or using cached files.

## Local Writes

Local writes use a temporary file in the destination directory and then rename
into place.

This avoids publishing partial files as final cache entries.

## Remote Writes

Remote writes should upload to a temporary remote path and then publish to the
final path.

If an upload is interrupted, rerun:

```sh
git-sfs push
```

Existing valid remote files are skipped.

## Broken Symlinks

Check for broken or stale symlinks:

```sh
git-sfs verify
```

Repair local cache binding with `git-sfs setup`.

## Cache Corruption

If a cached file is corrupt, `git-sfs verify --with-integrity` reports it.

If the remote still has a valid copy, remove the corrupt cached file and pull:

```sh
git-sfs pull <path>
git-sfs verify
```

## Git Safety

Git-tracked symlink targets must be relative and point into `.git-sfs/cache`.

This prevents absolute machine-local cache paths from being committed.

## What git-sfs Does Not Protect

`git-sfs` v1 does not provide:

- encryption
- access control
- team file locking
- permissions tracking
- timestamp tracking
- automatic Git hooks

Use filesystem permissions, rclone configuration, and normal Git review for
those concerns.
