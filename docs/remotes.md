# Remotes

Remote storage uses the same layout as the local cache:

```text
files/sha256/ab/<full_hash>
```

This makes remotes inspectable with ordinary tools:

```sh
rclone lsf remote-name:datasets/project/files
```

## rclone

git-sfs relies entirely on rclone for remote operations. The backend type,
credentials, and connection details all live in rclone's own config. git-sfs
config only says which named rclone remote to use and where within it to store
files.

Primary form — named remote with a path:

```toml
[remotes.default]
backend = "remote-name"
path = "datasets/project"
config = "rclone.conf"
```

`backend` is the rclone remote name as defined in rclone's config. `path` is the
directory within that backend where git-sfs stores files. `path` is optional;
omitting it uses the root of the rclone backend.

`config` is optional. When set to a relative path it is resolved from `.git-sfs`,
so `config = "rclone.conf"` uses `.git-sfs/rclone.conf`. Commit that file only
when it contains shareable, non-secret rclone settings. Keep tokens, passwords,
and machine-local credentials in each user's normal rclone config instead.

## Multiple remotes

Name any number of remotes. `push`, `pull`, and `verify` use `default` unless
you pass `-r`:

```toml
[remotes.default]
backend = "primary"
path = "datasets/project"

[remotes.backup]
backend = "backup"
path = "datasets/project"
```

```sh
git-sfs push -r backup
git-sfs pull -r backup
git-sfs verify -r backup
```

## Remote Safety

Remote writes:

- upload to a temporary path first
- publish to the final path with rename when possible
- never accept corrupt bytes as final files
- skip existing valid files
- are safe to retry after interruption
