# Development

Use `just` for common commands:

```sh
just --list
just fmt
just test
just smoke
just coverage
just release-snapshot
just check
```

## Local Go Paths

The `Justfile` defaults to:

```text
/Users/vadstup/.local/go/bin/go
```

It also uses writable caches in this workspace:

```text
/private/tmp/merk-go-cache
/private/tmp/merk-go-modcache
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

Run smoke test:

```sh
just smoke
```

Run coverage:

```sh
just coverage
```

Coverage is reported for visibility. Do not add tests only to raise the number.

## Release Snapshot

Build local release archives:

```sh
just release-snapshot
```

Expected archives:

```text
dist/merk-snapshot-darwin-amd64.tar.gz
dist/merk-snapshot-darwin-arm64.tar.gz
dist/merk-snapshot-linux-amd64.tar.gz
dist/merk-snapshot-linux-arm64.tar.gz
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
