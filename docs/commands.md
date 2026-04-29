# Commands

## merk init

Create initial project files:

```sh
merk init
```

Creates:

```text
dataset.yaml
.ds/
.gitignore entry for .ds/
```

It does not overwrite an existing `dataset.yaml` unless forced:

```sh
merk init --force
```

## merk setup

Prepare local machine state:

```sh
merk setup
```

Responsibilities:

- resolve cache path
- create `.ds/`
- create `.ds/worktree/`
- create cache directories
- validate `dataset.yaml`
- repair `.ds/worktree` symlinks when cached files exist

## merk add

Add one file:

```sh
merk add data/train-000.tar.zst
```

Add a directory recursively:

```sh
merk add data/
```

For each regular file, `merk`:

- hashes bytes with SHA-256
- stores bytes in the cache
- replaces the file with a relative symlink
- creates or repairs the `.ds/worktree` symlink

## merk status

Report repository state:

```sh
merk status
```

Reports problems such as:

- unconverted regular files
- broken Git symlinks
- missing cached files
- corrupt cached files
- missing `.ds/worktree` symlinks
- stale `.ds/worktree` symlinks
- invalid config

Output starts with stable category counts:

```text
tracked symlinks: 2
unconverted files: 0
broken git symlinks: 0
missing cache files: 0
corrupt cache files: 0
missing worktree symlinks: 0
stale worktree symlinks: 0
invalid config: 0
```

When issues exist, a `details:` section follows:

```text
details:
missing cache file: data/train-000.tar.zst: <hash>
```

## merk verify

Strict CI-oriented verification:

```sh
merk verify
```

Returns non-zero on failure.

On failure, `merk verify` prints the same category-count report as
`merk status`.

## merk push

Upload referenced cached files to the remote:

```sh
merk push
merk push backup
```

`merk push` skips files that already exist remotely and verify correctly.

## merk pull

Download missing files required by symlinks:

```sh
merk pull
merk pull data/train-000.tar.zst
merk pull data/
```

Downloaded bytes are hash-verified before being accepted.

When a path is provided, only symlinks below that path are considered. This is
the intended way to partially pull a dataset from the remote.

## merk materialize

Create or repair local `.ds/worktree` symlinks:

```sh
merk materialize
merk materialize data/
```

This does not copy or modify cached bytes.

## merk dematerialize

Remove local `.ds/worktree` symlinks:

```sh
merk dematerialize
merk dematerialize data/
```

This does not delete cached bytes.

## merk gc

Show unused local state:

```sh
merk gc --dry-run
```

Remove unused worktree symlinks:

```sh
merk gc --worktree-only
```

Remove unreferenced cached files:

```sh
merk gc --files
```

`merk gc` must never delete files referenced by the current Git symlink tree.
