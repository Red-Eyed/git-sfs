# Commands

Global flags:

```sh
git-sfs --verbose push
```

`--verbose` prints remote subprocess commands to stderr, which is useful when
debugging `ssh`, `rsync`, or `rclone` remotes.

## git-sfs init

Create initial project files, including a commented `.git-sfs/config.toml` starter file:

```sh
git-sfs init
```

Creates:

```text
.git-sfs/config.toml
.git-sfs/
.gitignore entries for .git-sfs/cache and .git-sfs/.cache
```

It does not overwrite an existing `.git-sfs/config.toml` unless forced:

```sh
git-sfs init --force
```

## git-sfs setup

Prepare local machine state:

```sh
git-sfs setup
```

Responsibilities:

- resolve cache path
- create `.git-sfs/`
- create or read `.git-sfs/cache`
- create cache directories
- validate `.git-sfs/config.toml`
- verify `.git-sfs/cache` reaches the cache

## git-sfs add

Add one file:

```sh
git-sfs add data/train-000.tar.zst
```

Add a directory recursively:

```sh
git-sfs add data/
```

For each regular file, `git-sfs`:

- hashes bytes with SHA-256
- stores bytes in the cache
- replaces the file with a relative symlink

## git-sfs import

Import an external file into the cache and create a symlink inside the repository:

```sh
git-sfs import /mnt/incoming/train-000.tar.zst data/train-000.tar.zst
```

Import an external directory recursively:

```sh
git-sfs import /mnt/incoming/dataset data/dataset
```

`git-sfs import` is for very large data where making a temporary repository copy is too expensive. It hashes each source file, moves it into the cache, verifies the cached bytes, and creates the destination symlink. When the source and cache are on the same filesystem this uses rename; across filesystems it falls back to copy-verify-remove.

By default, source symlinks are rejected. To follow source symlinks and import
the files they resolve to:

```sh
git-sfs import -L /mnt/incoming/dataset data/dataset
```

## git-sfs status

Report repository state:

```sh
git-sfs status
```

Reports problems such as:

- unconverted regular files
- broken Git symlinks
- missing cached files
- corrupt cached files
- invalid config

Output starts with stable category counts:

```text
tracked symlinks: 2
unconverted files: 0
broken git symlinks: 0
missing cache files: 0
corrupt cache files: 0
invalid config: 0
```

When issues exist, a `details:` section follows:

```text
details:
missing cache file: data/train-000.tar.zst: <hash>
```

## git-sfs verify

Strict CI-oriented verification:

```sh
git-sfs verify
```

Returns non-zero on failure.

On failure, `git-sfs verify` prints the same category-count report as
`git-sfs status`.

## git-sfs push

Upload referenced cached files to the remote:

```sh
git-sfs push
git-sfs push backup
```

`git-sfs push` skips files that already exist remotely and verify correctly.

## git-sfs pull

Download missing files required by symlinks:

```sh
git-sfs pull
git-sfs pull data/train-000.tar.zst
git-sfs pull data/
```

Downloaded bytes are hash-verified before being accepted.

When a path is provided, only symlinks below that path are considered. This is
the intended way to partially pull a dataset from the remote.

## git-sfs materialize

Verify local `.git-sfs/cache` can reach selected cached files:

```sh
git-sfs materialize
git-sfs materialize data/
```

This does not copy or modify cached bytes.

## git-sfs dematerialize

No-op compatibility command for the direct `.git-sfs/cache` layout:

```sh
git-sfs dematerialize
git-sfs dematerialize data/
```

This does not delete cached bytes.

## git-sfs gc

Show unreferenced cached files:

```sh
git-sfs gc --dry-run
```

Remove unreferenced cached files:

```sh
git-sfs gc --files
```

`git-sfs gc` must never delete files referenced by the current Git symlink tree.
