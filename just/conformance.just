# The conformance harness (test/differential, test/workflows).
#
# Recipes here drive the release binary at ./target/release/git-sfs by default.
# The underlying scripts still accept --binary NAME=PATH for explicit
# compatibility checks against older binaries.
#
# Long rationale sits in a detached comment block above each recipe; the [doc]
# attribute carries the one-liner, because `just --list` otherwise shows only the
# final comment line and reads as a non-sequitur.

# Drives the installed artifact through the real install path -- building a
# release tarball, running install.sh against a file:// endpoint, resolving from
# PATH. No in-tree integration test reaches any of that.

[group('conformance')]
[doc('shell workflow suite, through the real install path')]
workflows: build
    env GIT_SFS_BIN=./target/release/git-sfs bash test/workflows/run.sh

# Self-check: the same binary compared against itself must always agree, which
# guards the harness against nondeterminism. Pass two --binary flags to run.py
# directly to compare different implementations.

[group('conformance')]
[doc('state diff between two binaries (self-check by default)')]
differential: build
    python3 test/differential/run.py --binary a=./target/release/git-sfs --binary b=./target/release/git-sfs

# The lock protocol is cross-process state. Pass a second --binary to exercise
# compatibility with another released binary.

[group('conformance')]
[doc('lock protocol conformance across processes')]
lock-contention: build
    python3 test/differential/lock_contention.py --binary current=./target/release/git-sfs

# SIGINT mid-transfer must publish no partial file and must leave the operation
# retryable. Asserts invariants rather than diffing trees, since an interrupt
# lands at a point no binary controls.

[group('conformance')]
[doc('cancellation safety: nothing partial, nothing unrecoverable')]
cancellation: build
    python3 test/differential/cancellation.py --binary current=./target/release/git-sfs

# Cache object write bits are a storage invariant. This covers what happens when
# the bits no longer match the bytes' integrity.

[group('conformance')]
[doc('mode/content disagreement on cache objects')]
mode-preservation: build
    python3 test/differential/mode_preservation.py --binary current=./target/release/git-sfs

# Older binaries must be able to read stable state. Pass both binaries to
# exercise a real handoff in both directions; with one it is a self-check of
# every handoff step.

[group('conformance')]
[doc('downgrade invariant: state written by one binary, read by the other')]
downgrade: build
    python3 test/differential/downgrade.py --binary current=./target/release/git-sfs

# Fails on a stale claim, not on a known gap. Add --gate when every stable
# behavior must have explicit coverage.

[group('conformance')]
[doc('stable behavior coverage map')]
spec-coverage:
    python3 test/differential/coverage.py

# Deliberately NOT part of `check`: minutes rather than seconds. Pass a second
# --binary to compare implementations side by side; absolute times are
# machine-specific.

[group('conformance')]
[doc('performance baselines (nightly tier, not in check)')]
perf: build
    python3 test/differential/benchmark.py --binary current=./target/release/git-sfs

# One binary under two names must come out at ratio ~1.0. Establishes the
# measurement noise floor any regression threshold has to clear.

[group('conformance')]
[doc('perf self-check: establishes the measurement noise floor')]
perf-selfcheck: build
    python3 test/differential/benchmark.py --binary a=./target/release/git-sfs --binary b=./target/release/git-sfs
