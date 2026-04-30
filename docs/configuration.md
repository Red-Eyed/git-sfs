# Configuration

`git-sfs` has one tracked config file and one local cache symlink.

## .git-sfs/config.toml

`.git-sfs/config.toml` is committed to Git. `git-sfs init` writes a commented starter file so the important choices are visible without opening the docs:

```toml
version = 1

[remotes.default]
type = "rsync"
url = "user@host:/mnt/datasets/project"

[settings]
algorithm = "sha256"
```

Allowed here:

- project config version
- remote names
- remote types
- remote URLs
- shared settings

Supported remote types are `filesystem`, `rsync`, `ssh`, and `rclone`.

Forbidden here:

- local cache paths
- secrets
- tokens
- machine-local absolute paths
- temporary state

## .git-sfs/cache

`.git-sfs/cache` is not committed. It is a symlink to the real local cache.

By default, `git-sfs init` creates:

```text
.git-sfs/cache -> .git-sfs/.cache
```

To use an external cache, bind it during init or setup:

```sh
git-sfs --cache /mnt/shared/git-sfs-cache init
git-sfs --cache /mnt/shared/git-sfs-cache setup
```

Cache path priority:

```text
--cache
GIT_SFS_CACHE
.git-sfs/cache
```

## Ignored Local State

Only local cache state under `.git-sfs/` is ignored by Git:

```gitignore
.git-sfs/cache
.git-sfs/.cache
```
