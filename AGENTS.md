# AGENTS.md

## Project

`git-sfs` is a Go CLI for storing large file bytes outside Git while Git tracks symlinks.

Project direction:

- Keep `git-sfs` as small as possible.
- Treat it as a layer on top of Git, the filesystem, and well-known file movers.
- Prefer plain files, symlinks, directories, and subprocess calls over custom state.
- Supported remote tools should stay boring and familiar: `rsync`, `ssh`, and `rclone`.
- Do not add manifests, databases, background services, custom protocols, or hidden metadata.
- If a feature needs a new internal format, first ask whether Git or the filesystem already provides the needed state.

Core model:

- Git tracks `.git-sfs/config.toml` and relative symlinks.
- Git symlinks point into `.git-sfs/cache/files/sha256/<prefix>/<hash>`.
- `.git-sfs/cache` is a symlink to the cache root.
- Cache files live at `<cache>/files/sha256/<prefix>/<hash>`.
- Cache path is local state and must never be committed.

## Commands

Use a sandbox-writable Go cache when running here:

```sh
GOCACHE=/private/tmp/git-sfs-go-cache
```

Required checks after code changes:

```sh
just check
```

If `just` is unavailable, run the commands in `Justfile` manually.

Common user request: when asked to commit, push, and update version after changes, stage the finished changes, commit them with a focused message, create the next sequential version tag unless a specific version is named, push `main`, and push the new tag.

## Style

- Keep the implementation boring and explicit.
- Prefer standard library code unless a dependency is clearly worth it.
- Keep CLI parsing thin; put behavior in internal packages.
- Keep Git and filesystem state as the source of truth.
- Use temp files plus atomic rename for file writes.
- Hash-verify bytes before accepting cache or remote files.
- Comments are welcome when they explain invariants, safety behavior, or non-obvious control flow.
- Avoid comments that merely restate a line of code.

## Tests

Keep all of these healthy:

- Package unit tests under `internal/...`
- Workflow integration tests in `internal/core/app_test.go`
- CLI smoke test in `scripts/smoke.sh`
- GitHub Actions CI in `.github/workflows/ci.yml`
- GitHub release automation in `.github/workflows/release.yml`

Coverage is reported for visibility, but do not add tests only to increase the
percentage. Prefer behavior tests that prove a user-facing invariant or a real
failure mode. Filesystem tests should use `t.TempDir()` and should exercise git-sfs
behavior, not Go standard-library behavior such as `os.MkdirAll` failing when a
parent path is a file.

When changing storage, symlink, cache, or remote behavior, add or update tests for:

- Correct symlink target format
- Missing cache file detection
- Corrupt cache or remote file rejection
- Pull after cache file removal
- Push skipping existing remote files
- Retry-safe temp-file behavior where practical

## Documentation

`README.md` should stay user-focused:

- What the project does
- Why users should care
- Install command
- Supported platforms
- Quick start
- Config and command examples

Do not put secrets, local absolute cache paths, or temporary state in committed config examples except as clearly illustrative placeholders.
