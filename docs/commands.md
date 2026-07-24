# Commands

Global flags may appear before or after the command name — both
`git-sfs --verbose push` and `git-sfs push --verbose` work:

```sh
git-sfs push --verbose
git-sfs pull -j 8 data/
git-sfs --version
```

`--verbose` prints command debug output to stderr, including remote subprocess
commands when a remote backend is involved.

`-j`, `--jobs` sets the maximum number of parallel workers for commands that
process many files (`add`, `import`, `setup`, `verify`, `pull`). It overrides
`[settings].n_jobs` from the config; `0` (the default) means auto.

`--quiet` silences normal output, including the local hashing progress bar and
rclone's transfer progress during `push`/`pull`.

`--version` prints the `git-sfs` release version from the build tag.

Run `git-sfs <command> --help` for the flags specific to a command.

## git-sfs init

Create initial project files, including a commented `.git-sfs/config.toml` starter file:

```sh
git-sfs init
```

Creates:

```text
.git-sfs/config.toml
.git-sfs/
.git/sfs/cache/
.gitignore entries for .git-sfs/cache and .git-sfs/.cache
```

It does not overwrite an existing `.git-sfs/config.toml` unless forced:

```sh
git-sfs init --force
```

## git-sfs setup

Prepare local machine state:

```sh
git-sfs setup
```

Responsibilities:

- resolve cache path
- create `.git-sfs/`
- create or read `.git-sfs/cache`
- create cache directories
- validate `.git-sfs/config.toml`
- verify `.git-sfs/cache` reaches the cache

## git-sfs add

Add one file:

```sh
git-sfs add data/train-000.tar.zst
```

Add a directory recursively:

```sh
git-sfs add data/
```

For each regular file, `git-sfs`:

- hashes bytes with SHA-256
- stores bytes in the cache
- replaces the file with a relative symlink

When `.git-sfs/config.toml` sets `[settings].n_jobs`, `git-sfs add` hashes and
stores files with that worker limit before rewriting the repo paths.

## git-sfs import

Import an external file into the cache and create a symlink inside the repository:

```sh
git-sfs import /mnt/incoming/train-000.tar.zst data/train-000.tar.zst
```

Import an external directory recursively:

```sh
git-sfs import /mnt/incoming/dataset data/dataset
```

`git-sfs import` hashes each source file, copies it into the cache, verifies the cached bytes, and creates the destination symlink. The source is left intact by default — use `--move` to consume it instead.

**`--move`** — delete source files after caching. On the same filesystem this uses a rename (fast, atomic); across filesystems it falls back to copy-verify-remove. Use this when disk space is tight or you want to avoid keeping two copies.

```sh
git-sfs import --move /mnt/incoming/dataset data/dataset
```

When `.git-sfs/config.toml` sets `[settings].n_jobs`, unique source files are
prepared with that worker limit before destination symlinks are written.

By default, source symlinks are rejected. To follow source symlinks and import
the files they resolve to:

```sh
git-sfs import -L /mnt/incoming/dataset data/dataset
```

## git-sfs mv

Move a git-sfs symlink to a new location inside the repository:

```sh
git-sfs mv data/old/blob.bin data/new/blob.bin
git-sfs mv data/blob.bin datasets/      # places blob.bin inside datasets/
```

`git-sfs mv` rewrites the relative symlink target for the new path. The cache is not touched. Use this instead of `git mv` whenever the source and destination are at different directory depths — `git mv` preserves the old relative target verbatim, breaking the symlink.

## git-sfs verify

Strict CI-oriented verification:

```sh
git-sfs verify
git-sfs verify data/train-000.tar.zst
git-sfs verify data/validation/
git-sfs verify --with-integrity data/validation/
git-sfs verify --no-check-remote
git-sfs verify -r backup
git-sfs verify --rehash
git-sfs verify --rehash --rehash-sample 500
```

Returns non-zero on failure.

By default, `git-sfs verify` checks that tracked cache entries are present
locally and that tracked hashes are present on the configured default remote, so
another machine can pull and materialize the same symlinks.

`-r remote` checks against a named remote instead of `default`.

Remote checks use `[settings].n_jobs` when it is set. `0` means auto.

`--with-integrity` additionally recalculates hashes for local cache files and
remote files. This is slower, but it catches corruption instead of checking only
presence.

`--rehash` re-hashes every file in the local cache directory (not just
symlink-tracked files) to detect silent bit rot. It ignores the path argument and
walks all of `cache/files/sha256/`. Use `--rehash-sample N` to check only `N`
randomly chosen cache files — useful for periodic spot-checks on large caches
where re-hashing everything is too slow.

On failure, `git-sfs verify` prints stable category counts followed by a
`details:` section for each problem.

When a path is provided, only files and symlinks below that path are checked.
This keeps verification practical for partial-download workflows.

## git-sfs status

Show tracked files, their sizes, and where each one lives — **without
downloading any bytes**:

```sh
git-sfs status
git-sfs status data/
git-sfs status --remote default
git-sfs status --remote backup --json data/
```

For every tracked symlink, `git-sfs status` reports the file size, whether it is
cached locally, and (when a remote is given) whether it is present on that
remote. Size comes from the local cache file when present; otherwise, with a
remote, it is read from the remote's metadata (`rclone lsjson`) so you can learn
a file's size before ever pulling it.

By default `status` is local-only and makes no network calls.

`--remote NAME` checks presence and sizes against the named remote
(`--remote default` for the default remote). Supplying it both selects the
remote and turns on the remote check; it only reads remote metadata and never
transfers file bytes. Run `git-sfs remotes` to see the configured names.

`--json` emits a machine-readable summary plus one record per tracked symlink,
for scripting.

When a path is provided, only symlinks below that path are inspected.

Counts are aggregated over unique file contents, so files shared by several
symlinks are counted and sized once. Unlike `verify`, `status` is informational
and always exits `0`.

## git-sfs remotes

List the remotes configured in `.git-sfs/config.toml`:

```sh
git-sfs remotes
git-sfs remotes --json
```

`git-sfs remotes` prints each configured remote's name, backend, path, and
rclone config file, and marks the one named `default` (used by `push`, `pull`,
and `verify` when no `-r` is given). It reads only the committed config and never
contacts a backend — use `git-sfs doctor` to test connectivity.

`--json` emits the same list as machine-readable records.

## git-sfs push

Upload referenced cached files to the remote:

```sh
git-sfs push
git-sfs push data/train-000.tar.zst
git-sfs push data/
git-sfs push -r backup data/
```

`-r remote` pushes to a named remote instead of `default`.

When a path is provided, only symlinks below that path are uploaded. This is the
intended way to push part of a dataset, and it is required when the repository is
only partially pulled: subtrees you never pulled are dangling symlinks, and a
whole-repo push fails on them with `missing cached file`.

`--skip-missing` uploads the files that are cached instead of failing on the
first one that is not:

```sh
git-sfs push --skip-missing
```

Use it when the missing files are scattered rather than confined to a subtree,
so no single path argument selects the cached ones. It reports every skipped
path on stderr:

```
git-sfs: warning: push skipped 1 file(s) referenced by 3 symlink(s); the remote is not a complete copy
  a/blob (8446e508a4da)
  b/blob (8446e508a4da)
  c/blob (8446e508a4da)
  run: git-sfs pull <path> to restore them
```

The two counts differ when several symlinks share one cached file. Listing is
capped at 10 paths; `git-sfs status` prints the full set.

It is off by default on purpose. A push that omits files still exits `0`, so the
remote silently becomes an incomplete copy — treat a remote written this way as
partial until a full `git-sfs push` succeeds, and do not clear a local cache on
the strength of it.

`git-sfs push` skips files that are already present on the remote with a
matching checksum. It uses rclone's `--checksum` flag, which compares against
the backend's native hash (e.g. MD5 on S3/GCS) where available, so a corrupt
but same-size remote file is detected and re-uploaded.
It uses `[settings].n_jobs` worker slots when configured.

## git-sfs pull

Download missing files required by symlinks:

```sh
git-sfs pull
git-sfs pull data/train-000.tar.zst
git-sfs pull data/
git-sfs pull -r backup data/
```

`-r remote` pulls from a named remote instead of `default`.

Downloaded bytes are hash-verified before being accepted.
Missing hashes are downloaded with `[settings].n_jobs` worker slots when configured.

When a path is provided, only symlinks below that path are considered. This is
the intended way to partially pull a dataset from the remote.

## git-sfs doctor

Run a series of configuration and connectivity checks:

```sh
git-sfs doctor
git-sfs doctor -r backup
```

`-r remote` checks only the named remote instead of all configured remotes.

Checks run in order. When a check fails, dependent checks are skipped:

1. git repository (is the current directory inside a git repo?)
2. git-sfs config (can `.git-sfs/config.toml` be parsed?)
3. git-sfs version (satisfies `min_git_sfs_version` if set)
4. cache config (is `.git-sfs/cache` bound, usually by `git-sfs setup`?)
5. cache directory (does it exist and is it writable?)
6. rclone binary (is `rclone` on `PATH`?)
7. rclone version (satisfies `min_rclone_version` if set)

Then for each configured remote (or the selected remote):

8. rclone config file (does the per-remote config file exist?)
9. remote backend (is the rclone backend reachable?)
10. remote path (does the configured remote path exist?)

`git-sfs doctor` exits non-zero if any check fails, so it can be used in CI setup scripts to verify the environment before running a workflow.

## git-sfs self update

Update the `git-sfs` binary and `rclone` to their latest releases:

```sh
git-sfs self update
```

Both binaries are updated in the directory where the running `git-sfs` executable lives (resolved through any symlinks). Each binary is replaced atomically — a temp file is written and renamed over the existing binary, so a partial download never leaves a broken installation. On Linux and macOS this is safe even while the binary is running, because the kernel holds the old inode open until the process exits.

Output while running:

```
checking git-sfs version... (1s)
downloading git-sfs v1.19.0  [##########----------]  18.3 MiB/36.1 MiB
git-sfs v1.18.0 → v1.19.0
checking rclone version... (1s)
rclone v1.68.2 already up to date
```

If a binary is already at the latest version it is left untouched. If the download or install step fails the error is reported explicitly with the URL and reason.

### Corporate environments

`git-sfs self update` honors the same env vars as the installer:

| Variable | Effect |
|----------|--------|
| `GIT_SFS_SSL_CERT_FILE` | Path to a custom CA bundle (highest priority) |
| `SSL_CERT_FILE` | Path to a custom CA bundle (fallback) |
| `CURL_CA_BUNDLE` | Path to a custom CA bundle (fallback) |
| `GIT_SFS_INSECURE_TLS=1` | Disable TLS verification (last resort; prints a warning) |
| `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` | Standard proxy vars, honored automatically |
| `GIT_SFS_REPO` | Override GitHub repo (default: `Red-Eyed/git-sfs`) |
| `GIT_SFS_RELEASE_BASE_URL` | Override release download base URL |
| `GIT_SFS_RELEASE_LATEST_URL` | Override latest-release redirect URL |
| `GIT_SFS_RCLONE_BASE_URL` | Override rclone download base URL |
