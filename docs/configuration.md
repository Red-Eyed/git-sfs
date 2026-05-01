# Configuration

`git-sfs` has one tracked config file and one local cache symlink.

## .git-sfs/config.toml

`.git-sfs/config.toml` is committed to Git. `git-sfs init` writes a commented starter file so the important choices are visible without opening the docs:

```toml
version = 1

[remotes.default]
type = "rclone"
host = "remote-name"
path = "datasets/project"
config = "rclone.conf"

[settings]
algorithm = "sha256"
n_jobs = 0
```

Allowed here:

- project config version
- remote names
- remote types
- remote URLs
- remote hosts
- remote paths
- rclone config paths
- shared settings

Supported remote types are `filesystem` and `rclone`.

`[settings]` currently supports:

- `algorithm = "sha256"`
- `n_jobs = 0`

`n_jobs` controls bounded parallel work for `add`, `import`, `push`, `pull`,
and remote-heavy `verify` checks. `0` means auto.

Forbidden here:

- local cache paths
- secrets
- tokens
- machine-local absolute paths
- temporary state

Relative rclone config paths are resolved from `.git-sfs`. For example,
`config = "rclone.conf"` uses `.git-sfs/rclone.conf`. Commit that file only
when it contains shareable, non-secret rclone settings.

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
