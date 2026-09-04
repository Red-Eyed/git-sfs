# git-sfs Contract

This document defines the stable behavior a `git-sfs` binary must preserve.
Implementation details and human-readable wording may change, but the on-disk
layout, safety properties, and machine-observable outcomes below are release
contracts.

## Repository Layout

```text
<repo>/
  .git-sfs/
    config.toml        committed
    cache -> <abs>     symlink to cache root; not committed
  <tracked files>      relative symlinks into .git-sfs/cache/
```

`.git-sfs/cache` points at the canonicalized absolute cache root. Rebinding an
existing cache symlink to a different root is an error.

## Symlink Format

Git tracks ordinary relative symlinks. For a tracked file with hash `h`:

```text
<file> -> ../.git-sfs/cache/files/sha256/<h[0:2]>/<h>
```

The exact number of `..` components depends on the file's directory depth.
Validation rejects absolute targets, targets escaping the cache root, wrong
component counts, non-lowercase SHA-256 hashes, and mismatched prefix
directories.

`git-sfs mv` rewrites symlink targets for their new location and does not require
the referenced cache object to exist.

## Cache Layout

```text
<cache>/
  files/sha256/<2-char-prefix>/<64-char-sha256-hash>
  tmp/
  locks/
  trash/
```

Cache files are content-addressed, write-once, and stored read-only. A writable
cache object is unverified: commands must hash-check it before trusting it and
then restore the read-only mode.

All cache writes stage in the cache tree, never a system temp directory. Local
ingest and copy operations verify bytes before publication; publication is a
same-directory rename; parent directories are synced after rename where
durability matters.

## Remote Layout

Remote objects mirror the cache object layout:

```text
<remote>/files/sha256/<2-char-prefix>/<64-char-sha256-hash>
```

Push stages uploads under a remote temp prefix that is scoped to the repository
and process. Final remote objects are published only after the batch transfer and
basic verification pass.

Pull downloads into cache-local staging and atomically renames completed objects
into the cache. It trusts rclone by default and does not reread downloaded bytes.
`pull --verify` recalculates SHA-256 before adoption.

Every operation over a set of objects uses one rclone subprocess per distinct
transfer or verification phase, regardless of object count. Metadata queries
and transfers pass the complete set as exact relative paths with `--files-from`;
they must not spawn per object or per hash prefix, or enumerate the entire
remote object tree. A transient retry repeats the complete batch.

## Configuration

`.git-sfs/config.toml` is committed. It may contain remote definitions and shared
settings, but it must not contain local cache paths, secrets, tokens, or
temporary state.

Supported settings:

- `algorithm = "sha256"`
- `n_jobs`
- `retry_max`
- `min_rclone_version`
- `min_git_sfs_version`

The optional remote `config` field points at an rclone config file. Omit it to
use rclone's default config.

## Locks

Commands serialize with directory locks under `<cache>/locks`:

```text
add.lock
import.lock
setup.lock
pull.lock
push.lock
```

Each lock contains an `owner` file with the holding process id. A contended lock
is polled, cancellation stops the wait, and malformed owner files must not crash
the process.

## Exit And Output

Exit code `0` means success. Non-zero means the requested operation did not
complete successfully. Ctrl-C exits with `130` and reports `git-sfs: canceled`.

Human-readable wording is not a compatibility surface. Machine-readable JSON
shapes are documented by the commands that emit them.

## Integrity Rules

- Missing and corrupt are distinct outcomes.
- A remote error is not the same as an empty remote.
- A protected cache object may be trusted for fast paths, but `--rehash` and
  remote-integrity checks must read bytes and verify hashes.
- Pull trusts rclone's successful transfer result by default. `pull --verify`
  must reject a downloaded object whose bytes do not match its path hash.
- Commands must fail loudly rather than publish or accept incomplete state.

## Release Artifacts

Release archives are named:

```text
git-sfs-<tag>-darwin-amd64.tar.gz
git-sfs-<tag>-darwin-arm64.tar.gz
git-sfs-<tag>-linux-amd64.tar.gz
git-sfs-<tag>-linux-arm64.tar.gz
```

`<tag>` is a semantic release tag with a leading `v`, including prerelease
forms such as `v2.0.0-rc.1`.

`SHA256SUMS` accompanies every release and is verified before `self update`
replaces an installed binary.
