# Commands

Global flags:

```sh
git-sfs --verbose push
git-sfs --version
```

`--verbose` prints command debug output to stderr, including remote subprocess
commands when a remote backend is involved.

`--version` prints the `git-sfs` release version from the build tag.

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

When `.git-sfs/config.toml` sets `[settings].n_jobs`, `git-sfs add` hashes and
stores files with that worker limit before rewriting the repo paths.

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

When `.git-sfs/config.toml` sets `[settings].n_jobs`, unique source files are
prepared with that worker limit before destination symlinks are written.

By default, source symlinks are rejected. To follow source symlinks and import
the files they resolve to:

```sh
git-sfs import -L /mnt/incoming/dataset data/dataset
```

## git-sfs verify

Strict CI-oriented verification:

```sh
git-sfs verify
git-sfs verify data/train-000.tar.zst
git-sfs verify data/validation/
git-sfs verify --with-integrity data/validation/
git-sfs verify -r backup
```

Returns non-zero on failure.

By default, `git-sfs verify` checks that tracked cache entries are present
locally and that tracked hashes are present on the configured default remote, so
another machine can pull and materialize the same symlinks.

`-r remote` checks against a named remote instead of `default`.

Remote checks use `[settings].n_jobs` when it is set. `0` means auto.

`--with-integrity` additionally recalculates hashes for local cache files and
remote files. This is slower, but it catches corruption instead of checking only
presence.

On failure, `git-sfs verify` prints stable category counts followed by a
`details:` section for each problem.

When a path is provided, only files and symlinks below that path are checked.
This keeps verification practical for partial-download workflows.

## git-sfs push

Upload referenced cached files to the remote:

```sh
git-sfs push
git-sfs push -r backup
```

`-r remote` pushes to a named remote instead of `default`.

`git-sfs push` skips files that already exist remotely and verify correctly.
It uses `[settings].n_jobs` worker slots when configured.

## git-sfs pull

Download missing files required by symlinks:

```sh
git-sfs pull
git-sfs pull data/train-000.tar.zst
git-sfs pull data/
git-sfs pull -r backup data/
```

`-r remote` pulls from a named remote instead of `default`.

Downloaded bytes are hash-verified before being accepted.
Missing hashes are downloaded with `[settings].n_jobs` worker slots when configured.

When a path is provided, only symlinks below that path are considered. This is
the intended way to partially pull a dataset from the remote.

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

In dry-run mode, `git-sfs gc` prints each unreferenced file it would remove and a
stable summary count:

```text
would remove /path/to/cache/files/sha256/...
gc dry-run would remove 1 file(s)
```
