# Workflows

## Start A New Project

```sh
git init my-project
cd my-project
git-sfs --cache /mnt/shared/git-sfs-cache init
```

Edit `.git-sfs/config.toml` and set the remote.

```sh
git add .git-sfs/config.toml .gitignore
git commit -m "initialize git-sfs"
```

## Add A Single Large File

```sh
git-sfs add data/train-000.tar.zst
git add data/train-000.tar.zst
git commit -m "track train shard"
git-sfs push
```

## Add A Directory

```sh
git-sfs add data/
git add data/
git commit -m "track dataset"
git-sfs push
```

## Import Huge Data Without A Second Copy

Use `git-sfs import` when a file or tree already exists outside the repository and is too large to copy into place first:

```sh
git-sfs import /mnt/incoming/dataset data/dataset
git add data/dataset
git commit -m "track imported dataset"
git-sfs push
```

The command moves bytes into the cache and leaves symlinks under `data/dataset`. When the source and cache are on the same filesystem this uses `rename`; across filesystems it falls back to copy-verify-remove.

If the source path or files inside the source tree are symlinks, pass `-L` to
resolve them and import the files they point at:

```sh
git-sfs import -L /mnt/incoming/dataset data/dataset
```

## Clone And Pull Files

```sh
git clone <repo>
cd <repo>
git-sfs --cache /mnt/shared/git-sfs-cache setup
git-sfs pull
git-sfs verify
```

## Pull One File

```sh
git-sfs pull data/train-000.tar.zst
```

Only the cached file required by that Git symlink is downloaded.

## Pull One Directory

```sh
git-sfs pull data/train/
```

Only files referenced by symlinks under that directory are downloaded.

## Use A Temporary Cache

```sh
GIT_SFS_CACHE=/tmp/git-sfs-cache git-sfs setup
GIT_SFS_CACHE=/tmp/git-sfs-cache git-sfs pull data/sample.bin
```

## Use A Shared Machine Cache

```sh
git-sfs --cache /mnt/shared/git-sfs-cache setup
```

Multiple clones can use the same cache path if filesystem permissions allow it.

## Move To A New Cache

```sh
rm -f .git-sfs/cache
git-sfs --cache /new/cache/path setup
git-sfs pull
```

## Repair Broken Cache Binding

```sh
git-sfs setup
git-sfs materialize
git-sfs verify
```

If cached files exist, `.git-sfs/cache` can be rebound without downloading.

## Recover After Deleting .git-sfs

```sh
rm -rf .git-sfs/cache
git-sfs --cache /mnt/shared/git-sfs-cache setup
git-sfs materialize
git-sfs verify
```

## Recover After Cache Loss

```sh
git-sfs pull
git-sfs verify
```

If the remote has the files, missing cached files are downloaded again.

## Check In CI

```sh
git-sfs setup
git-sfs verify
```

Use a CI cache path through `GIT_SFS_CACHE`:

```sh
GIT_SFS_CACHE="$PWD/.git-sfs-cache" git-sfs setup
GIT_SFS_CACHE="$PWD/.git-sfs-cache" git-sfs verify
```

## Publish A Dataset Update

```sh
git-sfs add data/
git-sfs status
git add data/
git commit -m "update dataset"
git-sfs push
```

## Review What Git Will Track

```sh
git status
git diff --cached --stat
find data -type l -maxdepth 2 -print
```

Git should show symlinks and config, not large file bytes.

## Clean Local Materialization

```sh
git-sfs dematerialize data/
```

This leaves cached bytes in place. In the direct `.git-sfs/cache` layout there are no per-file local links to remove.

## Clean Unused Local Cache Files

First inspect:

```sh
git-sfs gc --dry-run
```

Then remove unreferenced cached files:

```sh
git-sfs gc --files
```

## Work With A Filesystem Remote

Useful for local testing or shared disks:

```toml
[remotes.default]
type = "filesystem"
url = "/mnt/datasets/project"
```

Then:

```sh
git-sfs push
git-sfs pull
```

## Work With An rsync Remote

```toml
[remotes.default]
type = "rsync"
url = "user@host:/mnt/datasets/project"
```

Then:

```sh
git-sfs push
git-sfs pull
```

## Work With An ssh Remote

```toml
[remotes.default]
type = "ssh"
url = "user@host:/mnt/datasets/project"
```

Then:

```sh
git-sfs push
git-sfs pull
```

## Work With An rclone Remote

First configure credentials with `rclone config`, then use the configured
remote name in `.git-sfs/config.toml`:

```toml
[remotes.default]
type = "rclone"
url = "remote-name:datasets/project"
```

`git-sfs` should call the installed `rclone` CLI and keep the same plain file
layout. It does not implement cloud-provider APIs directly.
