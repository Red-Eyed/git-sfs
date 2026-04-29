# Configuration

`merk` has one tracked config file and one optional local config file.

## dataset.yaml

`dataset.yaml` is committed to Git. `merk init` writes this as a commented starter file so the important choices are visible without opening the docs:

```yaml
version: 1

remotes:
  default:
    type: rsync
    url: user@host:/mnt/datasets/project

settings:
  algorithm: sha256
```

Allowed here:

- dataset version
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

## .ds/local.yaml

`.ds/local.yaml` is not committed:

```yaml
cache:
  path: /mnt/shared/merk-cache
```

The cache path is local to each machine.

## Cache Resolution

Cache path priority:

```text
--cache
MERK_CACHE
.ds/local.yaml
```

Examples:

```sh
merk --cache /mnt/cache setup
MERK_CACHE=/mnt/cache merk pull
```

## Ignored Local State

`.ds/` must be ignored by Git:

```gitignore
.ds/
```
