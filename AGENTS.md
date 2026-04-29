# AGENTS.md

## Project

`merk` is a Go CLI for storing large file bytes outside Git while Git tracks symlinks.

Core model:

- Git tracks `dataset.yaml` and relative symlinks.
- Git symlinks point into `.ds/worktree/sha256/<prefix>/<hash>`.
- `.ds/worktree` symlinks point to cache objects.
- Cache objects live at `<cache>/objects/sha256/<prefix>/<hash>`.
- Cache path is local state and must never be committed.

## Commands

Use a sandbox-writable Go cache when running here:

```sh
GOCACHE=/private/tmp/merk-go-cache
```

Required checks after code changes:

```sh
just check
```

If `just` is unavailable, run the commands in `Justfile` manually.

## Style

- Keep the implementation boring and explicit.
- Prefer standard library code unless a dependency is clearly worth it.
- Keep CLI parsing thin; put behavior in internal packages.
- Use temp files plus atomic rename for object writes.
- Hash-verify bytes before accepting cache or remote objects.
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
failure mode. Filesystem tests should use `t.TempDir()` and should exercise merk
behavior, not Go standard-library behavior such as `os.MkdirAll` failing when a
parent path is a file.

When changing storage, symlink, cache, or remote behavior, add or update tests for:

- Correct symlink target format
- Missing cache object detection
- Corrupt cache or remote object rejection
- Pull after cache object removal
- Push skipping existing remote objects
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
