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

```yaml
remotes:
  default:
    type: filesystem
    url: /mnt/datasets/project
```

Good for:

- local development
- shared disks
- tests
- simple single-machine workflows

## rsync

Use an rsync-style destination:

```yaml
remotes:
  default:
    type: rsync
    url: user@host:/mnt/datasets/project
```

Good for:

- SSH-accessible servers
- simple Unix storage
- backup-friendly infrastructure

## ssh

Use SSH command behavior with the same remote path style:

```yaml
remotes:
  default:
    type: ssh
    url: user@host:/mnt/datasets/project
```

The current implementation delegates file transfer behavior through the command
backend and should remain inspectable as plain files on the remote host.

## rclone

Planned backend:

```yaml
remotes:
  default:
    type: rclone
    url: remote-name:datasets/project
```

Rules for rclone support:

- call the installed `rclone` CLI
- keep the same `files/sha256/...` layout
- do not add cloud-specific SDKs
- do not add provider-specific config to `dataset.yaml`
- let users manage rclone credentials with rclone itself

## Remote Safety

Remote writes should:

- upload to a temporary path first
- publish to the final path with rename when possible
- never accept corrupt bytes as final files
- skip existing valid files
- be safe to retry after interruption
