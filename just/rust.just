# The Rust toolchain — the target implementation (rust-rewrite-plan.md). It
# grows into git-sfs proper as phases land; go.just disappears out from under
# it at the phase 7 cutover, and this file absorbs its recipe names then.
#
# Kept separate from go.just today because the two build different binaries
# under different toolchains, and `check`'s dependency list needs to name both
# independently until the cutover collapses them into one.

cargo := env_var_or_default("CARGO", "cargo")

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
[doc('everything the Rust workspace must pass')]
rust-check: rust-fmt rust-clippy rust-test rust-build
