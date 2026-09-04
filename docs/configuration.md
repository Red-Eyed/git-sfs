# Configuration

`git-sfs` has one tracked config file and one local cache symlink.

## .git-sfs/config.toml

`.git-sfs/config.toml` is committed to Git. `git-sfs init` writes a commented starter file so the important choices are visible without opening the docs:

```toml
version = 1

[remotes.default]
backend = "remote-name"
path = "datasets/project"

[settings]
algorithm = "sha256"
n_jobs = 0
```

Allowed here:

- project config version
- remote names and rclone target (backend, path, config)
- shared settings

`[settings]` currently supports:

- `algorithm = "sha256"` — only `sha256` is supported
- `n_jobs = 0` — cap for concurrent rclone transfers and pull adoption workers;
  `0` leaves their automatic/default choices in place
- `retry_max = 3` — retries per rclone call on transient failures
- `min_rclone_version = "1.67.0"` — fail fast if the installed rclone is older than this version
- `min_git_sfs_version = "1.6.0"` — fail fast if git-sfs itself is older than this version

`min_rclone_version` and `min_git_sfs_version` are enforced at the start of every command that uses a remote. Run `git-sfs doctor` to check both without performing an actual operation.

Forbidden here:

- local cache paths
- secrets
- tokens
- machine-local absolute paths
- temporary state

The optional `config` field points to an rclone config file. All remotes
typically share the same file — rclone.conf can define multiple backends and
each `[remotes.X]` section just names which backend to use. Omit `config` to use
rclone's default config (`~/.config/rclone/rclone.conf`). If `config` is set,
keep it outside `.git-sfs` unless the file is intentionally shareable and
contains no secrets.

## .git-sfs/cache

`.git-sfs/cache` is not committed. It is a symlink to the real local cache.

By default, `git-sfs init` creates:

```text
.git-sfs/cache -> .git/sfs/cache
```

To use an external cache, bind it during init or setup:

```sh
git-sfs init --cache /mnt/shared/git-sfs-cache
git-sfs setup --cache /mnt/shared/git-sfs-cache
```

After `init` or `setup`, normal commands use only the repo-facing
`.git-sfs/cache` symlink. `--cache` is a binding option for `init`/`setup`, not
a per-command override; environment variable cache overrides are not part of the
configuration model.

Existing repos keep working: if `.git-sfs/cache` already exists, setup preserves
it; if an old `.git-sfs/.cache` directory exists but the symlink is missing,
setup binds the symlink to that old cache instead of migrating bytes.

## Ignored Local State

Only compatibility/local cache handles under `.git-sfs/` are ignored by Git:

```gitignore
.git-sfs/cache
.git-sfs/.cache
```

New local state lives under `.git/sfs/`, which is already outside the working
tree and is not affected by `git clean`.
