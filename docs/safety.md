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

## Cancellation

Long-running operations are cancelable with `Ctrl-C` (SIGINT). The hash, copy,
and download loops check for cancellation on every chunk, so an interrupt stops
work promptly instead of running to completion.

Cancellation is safe: in-progress writes go to a temporary file that is only
renamed into place once complete, so an interrupted `add`, `import`, or `pull`
never publishes a partial cache file. Rerun the command to resume — already-valid
files are skipped.

An interrupted run reports `canceled` and exits with status `130`.

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
This re-hashes every cache file referenced by a tracked symlink.

For cache-wide bit rot detection (including orphaned files not referenced by any
symlink), use `--rehash`:

```sh
git-sfs verify --rehash               # re-hash every file in the cache
git-sfs verify --rehash --rehash-sample 500  # spot-check 500 random files
```

If the remote still has a valid copy, remove the corrupt cached file and pull:

```sh
git-sfs pull <path>
git-sfs verify
```

## Git Safety

Git-tracked symlink targets must be relative and point into `.git-sfs/cache`.

This prevents absolute machine-local cache paths from being committed.

## What git-sfs Does Not Protect

`git-sfs` does not provide:

- encryption
- access control
- team file locking
- permissions tracking
- timestamp tracking
- automatic Git hooks

Use filesystem permissions, rclone configuration, and normal Git review for
those concerns.
