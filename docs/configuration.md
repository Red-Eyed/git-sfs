# Configuration

`merk` has one tracked config file and one local cache symlink.

## .merk/config.toml

`.merk/config.toml` is committed to Git. `merk init` writes a commented starter file so the important choices are visible without opening the docs:

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

Forbidden here:

- local cache paths
- secrets
- tokens
- machine-local absolute paths
- temporary state

## .merk/cache

`.merk/cache` is not committed. It is a symlink to the real local cache.

By default, `merk init` creates:

```text
.merk/cache -> .merk/.cache
```

To use an external cache, bind it during init or setup:

```sh
merk --cache /mnt/shared/merk-cache init
merk --cache /mnt/shared/merk-cache setup
```

Cache path priority:

```text
--cache
MERK_CACHE
.merk/cache
```

## Ignored Local State

Only local cache state under `.merk/` is ignored by Git:

```gitignore
.merk/cache
.merk/.cache
```
