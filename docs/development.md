# Development

git-sfs is mid-rewrite from Go to Rust (see
[../docs/rust-rewrite-plan.md](rust-rewrite-plan.md)). `internal/` and `cmd/`
are the Go original; `crates/` is the Rust rewrite growing in alongside it.
Both are built and tested from the same `just check`.

Use `just` for common commands:

```sh
just --list          # grouped by go / rust / conformance / repo
just check           # everything CI runs, both toolchains
```

Recipes are split by lifetime rather than by size:

| File | Contents |
|---|---|
| `Justfile` | variables, `check`, repo chores |
| `just/go.just` | Go toolchain — retired at the Rust cutover |
| `just/rust.just` | Rust toolchain — the target implementation |
| `just/conformance.just` | the harness that decides whether a replacement is acceptable |

The conformance recipes all accept `--binary NAME=PATH` and default to the Go
binary, so the same commands drive a second implementation by pointing them
elsewhere — e.g. `./target/release/git-sfs` once a command is ported. See
[../test/differential/README.md](../test/differential/README.md).

## Rust

```sh
just rust-check      # fmt --check, clippy -D warnings, test, release build
just rust-build      # ./target/release/git-sfs
```

Or drive `cargo` directly from the workspace root — `cargo build --workspace`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets`. `cargo fmt`
rewrites files in place; re-read anything it touches before further edits.

## Local Go Paths

The `Justfile` defaults to the `go` binary on `PATH`.

Override when needed:

```sh
GO=/path/to/go just check
```

It also defaults to writable caches:

```text
<repo>/.cache/go-build
<repo>/.cache/go-mod
```

Override when needed:

```sh
GO=go GOCACHE="$PWD/.cache/go-build" GOMODCACHE="$PWD/.cache/go-mod" just check
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
