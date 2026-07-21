# Development

Use `just` for common commands:

```sh
just --list          # grouped by go / conformance / repo
just check           # everything CI runs
```

Recipes are split by lifetime rather than by size:

| File | Contents |
|---|---|
| `Justfile` | variables, `check`, repo chores |
| `just/go.just` | Go toolchain — retired at the Rust cutover |
| `just/conformance.just` | the harness that decides whether a replacement is acceptable |

The conformance recipes all accept `--binary NAME=PATH` and default to the Go
binary, so the same commands drive a second implementation by pointing them
elsewhere. See [../test/differential/README.md](../test/differential/README.md).

## Local Go Paths

The `Justfile` defaults to the `go` binary on `PATH`.

Override when needed:

```sh
GO=/path/to/go just check
```

It also defaults to writable caches:

```text
/private/tmp/git-sfs-go-cache
/private/tmp/git-sfs-go-modcache
```

Override when needed:

```sh
GO=go GOCACHE=/tmp/go-cache GOMODCACHE=/tmp/go-modcache just check
```

## Tests

Run all tests:

```sh
just test
```

Run workflow suite:

```sh
just workflows
```

Run coverage:

```sh
just coverage
```

Run benchmarks:

```sh
just bench
```

Coverage is reported for visibility. Do not add tests only to raise the number.

## Release Snapshot

Build local release archives:

```sh
just release-snapshot
```

Expected archives:

```text
dist/git-sfs-snapshot-darwin-amd64.tar.gz
dist/git-sfs-snapshot-darwin-arm64.tar.gz
dist/git-sfs-snapshot-linux-amd64.tar.gz
dist/git-sfs-snapshot-linux-arm64.tar.gz
```

Clean generated files:

```sh
just clean
```

## Commit Checklist

```sh
just check
git status --short
```

Before adding a dependency or a new file format, ask whether Git, the filesystem,
or an existing tool already solves the problem.
