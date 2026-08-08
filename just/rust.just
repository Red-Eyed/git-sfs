# The Rust toolchain recipes used by local development and CI.

cargo := env_var_or_default("CARGO", "cargo")

[group('repo')]
[doc('format check')]
fmt: rust-fmt

[group('repo')]
[doc('unit tests')]
test: rust-test

[group('repo')]
[doc('build ./target/release/git-sfs')]
build: rust-build

[group('repo')]
[doc('build release archives into dist/')]
release-snapshot: rust-release-snapshot

[group('repo')]
[doc('build the host release archive into dist/')]
release-native: rust-release-native

[group('rust')]
[doc('rustfmt check for the Rust workspace')]
rust-fmt:
    {{cargo}} fmt --all -- --check

[group('rust')]
[doc('clippy for the Rust workspace, warnings denied')]
rust-clippy:
    {{cargo}} clippy --workspace --all-targets -- -D warnings

[group('rust')]
[doc('Rust unit tests')]
rust-test:
    {{cargo}} test --workspace

[group('rust')]
[doc('build ./target/release/git-sfs')]
rust-build:
    {{cargo}} build --workspace --release

[group('rust')]
[doc('build release archives into dist/')]
rust-release-snapshot:
    sh scripts/build-rust-release.sh snapshot dist

[group('rust')]
[doc('build the host release archive into dist/')]
rust-release-native:
    sh scripts/build-rust-release.sh snapshot dist native

[group('rust')]
[doc('everything the Rust workspace must pass')]
rust-check: rust-fmt rust-clippy rust-test rust-build
