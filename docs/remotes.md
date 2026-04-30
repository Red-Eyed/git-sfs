# Remotes

Remote storage is plain files with the same layout as the local cache:

```text
files/sha256/ab/<full_hash>
```

This makes remotes inspectable with ordinary tools:

```sh
ssh user@host find /mnt/datasets/project/files -type f
rsync --list-only user@host:/mnt/datasets/project/files/
rclone lsf remote-name:datasets/project/files
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

## rsync

Use an rsync-style destination:

```toml
[remotes.default]
type = "rsync"
host = "user@host"
path = "/mnt/datasets/project"
```

Windows paths on an rsync target keep the drive letter after the host:

```toml
[remotes.default]
type = "rsync"
host = "user@host"
path = "D:/datasets/project"
```

To use a non-default SSH port without relying on SSH config:

```toml
[remotes.default]
type = "rsync"
host = "user@192.0.2.10:2222"
path = "D:/datasets/project"
```

Good for:

- SSH-accessible servers
- simple Unix storage
- backup-friendly infrastructure

## ssh

Use SSH command behavior with the same remote path style:

```toml
[remotes.default]
type = "ssh"
host = "storage"
path = "/mnt/datasets/project"
```

Windows paths on an SSH target use the same host/path split:

```toml
[remotes.default]
type = "ssh"
host = "user@host:2222"
path = "D:/datasets/project"
```

The current implementation delegates file transfer behavior through the command
backend and should remain inspectable as plain files on the remote host.

## rclone

Use any remote configured with the installed `rclone` CLI:

```toml
[remotes.default]
type = "rclone"
host = "remote-name"
path = "datasets/project"
```

Windows paths inside an rclone remote are passed through unchanged:

```toml
[remotes.default]
type = "rclone"
host = "remote-name"
path = "D:/datasets/project"
```

Rules for rclone support:

- call the installed `rclone` CLI
- keep the same `files/sha256/...` layout
- do not add cloud-specific SDKs
- do not add provider-specific config to `.git-sfs/config.toml`
- let users manage rclone credentials with rclone itself

## Remote Safety

Remote writes should:

- upload to a temporary path first
- publish to the final path with rename when possible
- never accept corrupt bytes as final files
- skip existing valid files
- be safe to retry after interruption
