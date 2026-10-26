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
just rust-build      # ./target/release/git-sfs
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

## Python wheels

Build all four macOS and manylinux wheels on a macOS host:

```sh
just build
```

Each wheel contains `git-sfs`, rclone, and the bundled licenses and sources.
The four wheels replace `dist/` only after every target builds successfully,
so a failed build preserves the previous output. Other old build artifacts in
`dist/` are removed on success. macOS is required for its Apple SDK.

To build and smoke-test only the host wheel:

```sh
sh scripts/build-python-wheel.sh dist/wheels native
sh test/release/test_python_wheel.sh dist/wheels/*.whl v2.0.0
```

The script requires Rust, Go 1.26 or newer, uv, curl, tar, and shasum. It downloads
checksum-pinned rclone source and vendored dependencies, builds without cgo,
includes upstream notices and corresponding source archives, and invokes
Maturin. uv supplies Maturin and Zig automatically, and rustup installs the
target's Rust standard library. Explicit targets are `linux/amd64`,
`linux/arm64`, `darwin/amd64`, and `darwin/arm64`. Build targets sequentially
because Maturin reads a shared staging directory.

The version comes from Cargo: `2.0.0` for this stable release. Maturin converts
prerelease spellings when needed; the executable retains the Git tag form,
such as `v2.0.0`.
When updating the rclone pin, update both archive hashes and the smoke test's
expected version, and review the new dependency notices. Complete rclone and
dependency sources accompany the notices, including LGPLv3 and MPL-covered
code, with instructions for rebuilding and replacing rclone.

CI builds and installs wheels on all four platforms. Tag releases attach them
to the GitHub release with SHA-256 checksums. **Only the maintainer publishes
to PyPI.** After `just build`, upload the wheels using your PyPI credentials:

```sh
uv publish dist/*.whl
```

Publish all four wheels together. Source distributions are not part of this
release path; users install prebuilt binaries without a compiler.

## Commit Checklist

```sh
just check
git status --short
```

Before adding a dependency or a new file format, ask whether Git, the filesystem,
or an existing tool already solves the problem.
