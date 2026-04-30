# Remotes

Remote storage is plain files with the same layout as the local cache:

```text
files/sha256/ab/<full_hash>
```

This makes remotes inspectable with ordinary tools:

```sh
rclone lsf remote-name:datasets/project/files
find /mnt/datasets/project/files -type f
```

## filesystem

Use a local or mounted directory:

```toml
[remotes.default]
type = "filesystem"
path = "/mnt/datasets/project"
```

Windows drive-letter paths are filesystem paths:

```toml
[remotes.default]
type = "filesystem"
path = "D:/datasets/project"
```

Good for:

- local development
- shared disks
- tests
- simple single-machine workflows

## rclone

Use any remote configured with the installed `rclone` CLI:

```toml
[remotes.default]
type = "rclone"
host = "remote-name"
path = "datasets/project"
config = "rclone.conf"
```

Windows paths inside an rclone remote are passed through unchanged:

```toml
[remotes.default]
type = "rclone"
host = "remote-name"
path = "D:/datasets/project"
config = "rclone.conf"
```

`config` is optional. When set to a relative path, it is resolved from
`.git-sfs`, so `config = "rclone.conf"` uses `.git-sfs/rclone.conf`.
Only commit that file when it contains shareable, non-secret rclone settings.
Keep tokens, passwords, and machine-local credentials in each user's normal
rclone config instead.

Rules for rclone support:

- call the installed `rclone` CLI
- keep the same `files/sha256/...` layout
- do not add cloud-specific SDKs
- keep provider config in rclone's own config format
- do not commit rclone secrets or tokens

## Remote Safety

Remote writes should:

- upload to a temporary path first
- publish to the final path with rename when possible
- never accept corrupt bytes as final files
- skip existing valid files
- be safe to retry after interruption
