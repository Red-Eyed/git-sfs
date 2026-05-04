# git-sfs

[![CI](https://github.com/Red-Eyed/git-sfs/actions/workflows/ci.yml/badge.svg)](https://github.com/Red-Eyed/git-sfs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/Red-Eyed/git-sfs/branch/main/graph/badge.svg)](https://codecov.io/gh/Red-Eyed/git-sfs)

`git-sfs` keeps large files — datasets, model checkpoints, media archives — out of
Git while Git tracks exactly where they belong.

```text
Git tracks symlinks.
git-sfs stores file bytes.
rclone moves files.
```

No LFS server. No database. No pointer files. Git commits normal symlinks;
the bytes live in a local content-addressed cache and sync to any rclone remote.

Use it when you want a repo that stays fast and cloneable, large files addressed
by SHA-256 hash, a remote you can inspect with `rclone ls`, and CI that fails
loudly when referenced files are missing.

## Install

```sh
curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

If `raw.githubusercontent.com` is blocked (corporate proxy), use the release asset URL instead:

```sh
curl -LsSf https://github.com/Red-Eyed/git-sfs/releases/latest/download/install.sh | sh
```

Prebuilt binaries for macOS and Linux (arm64 and x86_64). Installs `rclone` too if not already on `PATH`.
See [docs/installation.md](docs/installation.md) for proxy, CA bundle, and source-build options.

Build from source:

```sh
go build ./cmd/git-sfs
```

## Quick start

```sh
git-sfs init                    # create .git-sfs/config.toml
# edit config.toml: set remote backend, path, rclone config
git-sfs setup                   # bind local cache
git-sfs add data/               # hash files, replace with symlinks
git add .git-sfs/config.toml data/
git commit -m "track datasets"
git-sfs push                    # upload to remote
```

On another machine:

```sh
git clone <repo> && cd <repo>
git-sfs setup
git-sfs pull                    # download from remote
```

Files under `data/` open normally through symlinks.

## Documentation

- [Concepts](docs/concepts.md)
- [Installation](docs/installation.md)
- [Commands](docs/commands.md)
- [Configuration](docs/configuration.md)
- [Remotes](docs/remotes.md)
- [Workflows](docs/workflows.md)
- [Safety](docs/safety.md)
- [Development](docs/development.md)
