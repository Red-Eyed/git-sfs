---
name: git-sfs
description: >
  Use when working with git-sfs, large file storage, ML datasets, or any task
  involving .git-sfs directories, cache files, or rclone remotes. Covers all
  git-sfs commands: init, setup, add, import, mv, push, pull, verify, status,
  doctor, self update. Run git-sfs llms-txt for the full embedded reference.
---

# git-sfs skill

git-sfs stores large file bytes outside Git while Git tracks plain symlinks.
No LFS server, no pointer files, no database — just symlinks, a local
content-addressed cache, and rclone for remote sync.

## Load full reference

If `git-sfs` is installed, run:

```bash
git-sfs llms-txt
```

This prints the complete embedded reference — all commands, configuration,
workflows, and safety notes — without needing network access.

## Core model

```
Git tracks:      .git-sfs/config.toml  +  relative symlinks
git-sfs stores:  <cache>/files/sha256/<2-char-prefix>/<sha256-hash>
rclone syncs:    the same layout to any rclone backend
```

After `git clone`, a collaborator runs:

```bash
git-sfs setup   # bind local cache, restore symlinks
git-sfs pull    # download missing file bytes from remote
```

## Key commands

| Command | What it does |
|---------|--------------|
| `git-sfs init` | Create `.git-sfs/config.toml` starter file |
| `git-sfs setup` | Bind cache, restore symlinks — run after every clone |
| `git-sfs add <path>` | Hash files, cache bytes, replace with symlinks |
| `git-sfs import <src> <dst>` | Bring external files into tracking |
| `git-sfs push` | Upload cached files to rclone remote |
| `git-sfs pull [path]` | Download missing files from remote |
| `git-sfs verify [path]` | Integrity check; exits non-zero on failure (use in CI) |
| `git-sfs status [path]` | Show sizes and presence; always exits 0 |
| `git-sfs doctor` | Diagnose config and remote connectivity |
| `git-sfs self update` | Update git-sfs and rclone binaries |

## Common workflows

**New project:**
```bash
git-sfs init
# edit .git-sfs/config.toml — set backend, path, config
git-sfs setup
git-sfs add data/
git add .git-sfs/config.toml data/
git commit -m "track dataset"
git-sfs push
```

**After clone:**
```bash
git-sfs setup
git-sfs pull
```

**CI verification:**
```bash
git-sfs setup
git-sfs verify          # exits non-zero if any file is missing or corrupt
```

## Diagnosing problems

Run `git-sfs doctor` first — it checks git repo, config, cache, rclone binary,
rclone version, and remote connectivity in order, stopping at the first failure.

## Configuration shape

`.git-sfs/config.toml` is committed to Git. Never put secrets or local paths here.

```toml
version = 1

[remotes.default]
backend = "my-rclone-remote"
path    = "datasets/project"
config  = "rclone.conf"     # optional; relative to .git-sfs/

[settings]
n_jobs  = 0                 # 0 = auto parallelism
```
