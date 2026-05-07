# .git-sfs

This directory is managed by [git-sfs](https://github.com/Red-Eyed/git-sfs).

git-sfs stores large file bytes outside Git while Git tracks lightweight symlinks
pointing into a local cache. Do not edit the contents of this directory manually.

## Contents

| Path | Tracked by Git | Description |
|------|---------------|-------------|
| `README.md` | yes | This file |
| `config.toml` | yes | Dataset config: remote URL, rclone settings, concurrency |
| `rclone.conf` | optional | [rclone](https://rclone.org) remote config; commit only if it contains no secrets |
| `cache` | no | Symlink to the local cache root (machine-local) |
| `cache/files/sha256/<prefix>/<hash>` | no | Immutable cached file objects, named by content hash |
| `cache/tmp/` | no | Staging area for in-progress downloads and imports |
| `cache/locks/` | no | Advisory lock files used during concurrent operations |

## Full documentation

<https://github.com/Red-Eyed/git-sfs>
