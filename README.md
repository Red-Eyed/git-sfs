# git-sfs

[![CI](https://github.com/Red-Eyed/git-sfs/actions/workflows/ci.yml/badge.svg)](https://github.com/Red-Eyed/git-sfs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/Red-Eyed/git-sfs/branch/main/graph/badge.svg)](https://codecov.io/gh/Red-Eyed/git-sfs)

`git-sfs` stands for **Git Symbolic File Storage**. It is a small CLI for
keeping large files out of Git while keeping your repository simple, cloneable,
and understandable.

It is like Git LFS in spirit, but Git tracks normal symlinks instead of pointer
files. The large file bytes live in a local content-addressed cache and can be
synced to a remote with familiar tools such as `rsync`, `ssh`, or `rclone`.

```text
Git tracks symlinks.
git-sfs stores file bytes.
rsync/ssh/rclone moves files.
```

No LFS server. No database. No hidden manifest branch. No custom Git protocol.
`git-sfs` is meant to stay a thin layer over Git, the filesystem, and tools people
already know.

## Why git-sfs?

Git is excellent at source code and metadata. It is not excellent at multi-GB
datasets, model checkpoints, media dumps, build artifacts, or experiment blobs.

`git-sfs` gives you a boring, explicit way to keep those bytes outside Git while
still letting the Git tree describe exactly which files belong in the project.

Use `git-sfs` when you want:

- A repo that stays small and fast
- Large files addressed by SHA-256 content hash
- A cache path that is local to each machine and never committed
- A remote layout you can inspect with `ssh`, `rsync`, `rclone`, or `find`
- CI checks that fail when referenced files are missing or corrupt
- Another machine to clone the repo, run `git-sfs setup`, run `git-sfs pull`, and work

`git-sfs` is intentionally not a platform. It is a thin layer over Git symlinks,
plain files, local directories, and well-known file transfer tools.

## How It Works

Suppose you add a large file:

```text
data/train-000.tar.zst
```

`git-sfs add data/train-000.tar.zst` hashes the file, stores the bytes in your
local cache, and replaces the file with a Git-tracked symlink:

```text
data/train-000.tar.zst -> ../.git-sfs/cache/files/sha256/ab/<hash>
```

The local `.git-sfs/cache` symlink is untracked and points to the real cache root:

```text
.git-sfs/cache/files/sha256/ab/<hash> -> <cache>/files/sha256/ab/<hash>
```

Opening `data/train-000.tar.zst` follows `.git-sfs/cache` and reads the cached
file bytes.

Git stores the file list as ordinary directories and symlinks. The cache stores
the bytes. The remote stores the same SHA-256 file layout.

## Install

```sh
curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Prebuilt release binaries are published for:

```text
macOS arm64
macOS x86_64
Linux arm64
Linux x86_64
```

By default this installs `git-sfs` into:

```text
$HOME/.local/bin
```

You can override the install location:

```sh
GIT_SFS_INSTALL_DIR=/usr/local/bin curl -LsSf https://raw.githubusercontent.com/Red-Eyed/git-sfs/main/scripts/install.sh | sh
```

Or build from source:

```sh
go build ./cmd/git-sfs
```

The installer detects macOS/Linux and arm64/x86_64 automatically.

## Quick Start

Create project metadata. This creates a commented `.git-sfs/config.toml` starter file you can edit in place:

```sh
git-sfs init
```

`git-sfs init` creates `.git-sfs/.cache` and `.git-sfs/cache` by default. To bind an external cache instead, pass a cache path:

```sh
git-sfs --cache /mnt/shared/git-sfs-cache init
```

Edit `.git-sfs/config.toml` and set your remote:

```toml
version = 1

[remotes.default]
type = "rsync"
url = "user@host:/mnt/datasets/project"

[settings]
algorithm = "sha256"
```

Initialize local state:

```sh
git-sfs setup
```

Add large files:

```sh
git-sfs add data/
```

Commit the metadata:

```sh
git add .git-sfs/config.toml .gitignore data/
git commit -m "track dataset files with git-sfs"
```

Upload files:

```sh
git-sfs push
```

On another machine:

```sh
git clone <repo>
cd <repo>
git-sfs --cache /mnt/shared/git-sfs-cache setup
git-sfs pull
```

The files under `data/` now open normally through symlinks.

You can also pull only the files you need:

```sh
git-sfs pull data/train-000.tar.zst
git-sfs pull data/validation/
```

## Commands

```sh
git-sfs init
git-sfs setup
git-sfs add <path>
git-sfs status
git-sfs verify
git-sfs push [remote]
git-sfs pull [path]
git-sfs materialize [path]
git-sfs dematerialize [path]
git-sfs gc --dry-run
git-sfs gc --files
```

Detailed command reference: [docs/commands.md](docs/commands.md)

## Configuration

`.git-sfs/config.toml` is committed to Git:

```toml
version = 1

[remotes.default]
type = "rsync"
url = "user@host:/mnt/datasets/project"

[settings]
algorithm = "sha256"
```

It must not contain cache paths, secrets, tokens, or machine-local state.

`.git-sfs/cache` is a Git-ignored symlink to the real local cache. By default, `git-sfs init` creates `.git-sfs/.cache` and points `.git-sfs/cache` at it.

Cache resolution order:

```text
--cache
GIT_SFS_CACHE
.git-sfs/cache
```

Detailed configuration reference: [docs/configuration.md](docs/configuration.md)

## Remote Storage

Remote storage uses the same content-addressed file layout as the local cache:

```text
files/sha256/ab/<full_hash>
```

The first supported remote styles are:

```text
rsync
ssh
filesystem
```

This keeps the remote easy to inspect, back up, mirror, or repair with ordinary
Unix tools.

Remote details: [docs/remotes.md](docs/remotes.md)

## Documentation

- [Concepts](docs/concepts.md)
- [Installation](docs/installation.md)
- [Configuration](docs/configuration.md)
- [Commands](docs/commands.md)
- [Workflows](docs/workflows.md)
- [Remotes](docs/remotes.md)
- [Safety](docs/safety.md)
- [Development](docs/development.md)
- [Project status](docs/status.md)

## Safety

`git-sfs` is designed around retry-safe operations:

- Files are addressed by SHA-256
- Downloads are hash-verified before being accepted
- Corrupt cache files are detected
- Local cache paths are never written to Git-tracked config
- `.git-sfs/` is untracked and gitignored
- Missing and broken symlinks are reported by `git-sfs status` and `git-sfs verify`

For CI, run:

```sh
git-sfs verify
```

It exits non-zero if referenced files are missing, corrupt, or incorrectly
materialized.

`git-sfs status` and `git-sfs verify` print stable category counts before detailed
messages, so CI can match clear strings such as `missing cache files: 0`.

Safety details: [docs/safety.md](docs/safety.md)

## Project Status

`git-sfs` is early. The core local workflow, filesystem remote path, tests, smoke
test, and release automation are in place. The design intentionally favors a
small, auditable implementation over a large feature surface.

The goal is not to replace every large-file tool. The goal is to make the common
case boring:

```text
clone repo
configure cache
pull files
use files
```
