# Workflows

## Start A New Project

```sh
git init my-project
cd my-project
merk init
mkdir -p .ds
cat > .ds/local.yaml <<EOF
cache:
  path: /mnt/shared/merk-cache
EOF
merk setup
```

Edit `dataset.yaml` and set the remote.

```sh
git add dataset.yaml .gitignore
git commit -m "initialize merk"
```

## Add A Single Large File

```sh
merk add data/train-000.tar.zst
git add data/train-000.tar.zst
git commit -m "track train shard"
merk push
```

## Add A Directory

```sh
merk add data/
git add data/
git commit -m "track dataset"
merk push
```

## Clone And Pull Files

```sh
git clone <repo>
cd <repo>
mkdir -p .ds
cat > .ds/local.yaml <<EOF
cache:
  path: /mnt/shared/merk-cache
EOF
merk setup
merk pull
merk verify
```

## Pull One File

```sh
merk pull data/train-000.tar.zst
```

Only the cached file required by that Git symlink is downloaded.

## Pull One Directory

```sh
merk pull data/train/
```

Only files referenced by symlinks under that directory are downloaded.

## Use A Temporary Cache

```sh
MERK_CACHE=/tmp/merk-cache merk setup
MERK_CACHE=/tmp/merk-cache merk pull data/sample.bin
```

## Use A Shared Machine Cache

```sh
mkdir -p .ds
cat > .ds/local.yaml <<EOF
cache:
  path: /mnt/shared/merk-cache
EOF
merk setup
```

Multiple clones can use the same cache path if filesystem permissions allow it.

## Move To A New Cache

```sh
merk dematerialize
cat > .ds/local.yaml <<EOF
cache:
  path: /new/cache/path
EOF
merk setup
merk pull
```

## Repair Broken Worktree Symlinks

```sh
merk setup
merk materialize
merk verify
```

If cached files exist, materialization can be repaired without downloading.

## Recover After Deleting .ds

```sh
rm -rf .ds
mkdir -p .ds
cat > .ds/local.yaml <<EOF
cache:
  path: /mnt/shared/merk-cache
EOF
merk setup
merk materialize
merk verify
```

## Recover After Cache Loss

```sh
merk pull
merk verify
```

If the remote has the files, missing cached files are downloaded again.

## Check In CI

```sh
merk setup
merk verify
```

Use a CI cache path through `MERK_CACHE`:

```sh
MERK_CACHE="$PWD/.merk-cache" merk setup
MERK_CACHE="$PWD/.merk-cache" merk verify
```

## Publish A Dataset Update

```sh
merk add data/
merk status
git add data/
git commit -m "update dataset"
merk push
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
merk dematerialize data/
```

This leaves cached bytes in place and removes only local `.ds/worktree` links.

## Clean Unused Local Cache Files

First inspect:

```sh
merk gc --dry-run
```

Then remove unreferenced cached files:

```sh
merk gc --files
```

## Work With A Filesystem Remote

Useful for local testing or shared disks:

```yaml
remotes:
  default:
    type: filesystem
    url: /mnt/datasets/project
```

Then:

```sh
merk push
merk pull
```

## Work With An rsync Remote

```yaml
remotes:
  default:
    type: rsync
    url: user@host:/mnt/datasets/project
```

Then:

```sh
merk push
merk pull
```

## Work With An ssh Remote

```yaml
remotes:
  default:
    type: ssh
    url: user@host:/mnt/datasets/project
```

Then:

```sh
merk push
merk pull
```

## Planned rclone Remote

The intended shape is:

```yaml
remotes:
  default:
    type: rclone
    url: remote-name:datasets/project
```

`merk` should call the installed `rclone` CLI and keep the same plain file
layout. It should not implement cloud-provider APIs directly.
