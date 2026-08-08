# Development

git-sfs is a Rust workspace. The binary crate lives in `crates/git-sfs`; the
reusable command/core logic lives in `crates/git-sfs-core`.

Use `just` for common commands:

```sh
just --list          # grouped by rust / conformance / repo
just check           # everything CI runs
```

Recipes are split by lifetime rather than by size:

| File | Contents |
|---|---|
| `Justfile` | variables, `check`, repo chores |
| `just/rust.just` | Rust toolchain recipes |
| `just/conformance.just` | workflow and contract conformance harnesses |

The conformance scripts all accept `--binary NAME=PATH`; the `just` recipes
default to `./target/release/git-sfs`, the same artifact release builds use. See
[../test/differential/README.md](../test/differential/README.md).

## Rust

```sh
just check           # fmt --check, clippy -D warnings, test, release build, conformance
just build           # ./target/release/git-sfs
```

Or drive `cargo` directly from the workspace root — `cargo build --workspace`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets`. `cargo fmt`
rewrites files in place; re-read anything it touches before further edits.

## Tests

Run all tests:

```sh
just test
```

Run workflow suite:

```sh
just workflows
```

Run contract coverage:

```sh
just spec-coverage
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
