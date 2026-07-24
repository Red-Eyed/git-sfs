# Workflows

## Start A New Project

```sh
git init my-project
cd my-project
git-sfs init
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
git-sfs setup
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

## Use A Shared Machine Cache

```sh
git-sfs setup --cache /mnt/shared/git-sfs-cache
```

Multiple clones can use the same cache path if filesystem permissions allow it.

## Move To A New Cache

```sh
rm -f .git-sfs/cache
git-sfs setup --cache /new/cache/path
git-sfs pull
```

## Repair Broken Cache Binding

```sh
git-sfs setup
git-sfs verify
```

`setup` recreates `.git-sfs/cache` when the binding is missing. If the binding
already exists, it is preserved.

## Recover After Deleting .git-sfs

```sh
rm -rf .git-sfs/cache
git-sfs setup --cache /mnt/shared/git-sfs-cache
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

Bind the CI cache once, then run normal commands through `.git-sfs/cache`:

```sh
git-sfs setup --cache "$PWD/.git-sfs-cache"
git-sfs verify
```

## Publish A Dataset Update

```sh
git-sfs add data/
git add data/
git commit -m "update dataset"
git-sfs push
git-sfs verify data/
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
git-sfs setup
```

`setup` repairs the local cache binding and leaves cached bytes in place.

## Work With An rclone Remote

Define the remote in rclone's config, then reference it by name in
`.git-sfs/config.toml`. The backend type, credentials, and connection settings
live entirely in rclone — git-sfs only needs the remote name and the path within
it.

Example rclone config (`~/.config/rclone/rclone.conf`):

```ini
[myremote]
type = s3
provider = AWS
region = us-east-1
```

Corresponding git-sfs config:

```toml
[remotes.default]
backend = "myremote"
path = "datasets/project"
```

Omit `config` to use rclone's default config. Machine-local credentials stay in
each user's own rclone config.

```sh
git-sfs push
git-sfs pull
```
