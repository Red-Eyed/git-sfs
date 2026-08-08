# The conformance harness (test/differential, test/workflows).
#
# Every recipe here takes --binary NAME=PATH and defaults to the Go binary, so
# the same commands drive v2 by pointing them at ./target/release/git-sfs. That
# is deliberate: these outlive the Go implementation and are the acceptance gate
# for replacing it.
#
# Long rationale sits in a detached comment block above each recipe; the [doc]
# attribute carries the one-liner, because `just --list` otherwise shows only the
# final comment line and reads as a non-sequitur.

# Drives the installed artifact through the real install path -- building a
# release tarball, running install.sh against a file:// endpoint, resolving from
# PATH. No in-tree integration test reaches any of that.

[group('conformance')]
[doc('shell workflow suite, through the real install path')]
workflows:
    env PATH="$(dirname {{go}}):$PATH" GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} bash test/workflows/run.sh

# Self-check: the same binary compared against itself must always agree, which
# guards the harness against nondeterminism. Pass two --binary flags to run.py
# directly to compare different implementations.

[group('conformance')]
[doc('state diff between two binaries (self-check by default)')]
differential: build
    python3 test/differential/run.py --binary a=./git-sfs --binary b=./git-sfs

# contract-spec 8 makes the lock an inter-version contract: during migration a
# user runs v1 in one shell and v2 in another against one cache. Pass a second
# --binary to exercise real cross-version contention.

[group('conformance')]
[doc('lock protocol conformance across processes')]
lock-contention: build
    python3 test/differential/lock_contention.py --binary v1=./git-sfs

# SIGINT mid-transfer must publish no partial file and must leave the operation
# retryable. Asserts invariants rather than diffing trees, since an interrupt
# lands at a point no binary controls.

[group('conformance')]
[doc('cancellation safety: nothing partial, nothing unrecoverable')]
cancellation: build
    python3 test/differential/cancellation.py --binary v1=./git-sfs

# contract-spec 4.1: the read-only bit stands in for "hash-verified", so this
# covers what happens when it stops telling the truth.

[group('conformance')]
[doc('mode/content disagreement on cache objects')]
mode-preservation: build
    python3 test/differential/mode_preservation.py --binary v1=./git-sfs

# rust-rewrite-plan 7c: v1 must still operate anything v2 has touched. Pass both
# binaries to exercise the real handoff in both directions; with one it is a
# self-check of every handoff step.

[group('conformance')]
[doc('downgrade invariant: state written by one binary, read by the other')]
downgrade: build
    python3 test/differential/downgrade.py --binary v1=./git-sfs

# Fails on a stale claim, not on a known gap. Add --gate for the Phase 7
# criterion that no clause is UNCOVERED.

[group('conformance')]
[doc('contract-spec clause coverage map')]
spec-coverage:
    python3 test/differential/coverage.py

# rust-rewrite-plan 9b. Deliberately NOT part of `check`: minutes rather than
# seconds, and it belongs to the nightly tier. Pass a second --binary to compare
# implementations side by side, which is the only form the Phase 7 gate accepts
# -- absolute times are machine-specific.

[group('conformance')]
[doc('performance baselines (nightly tier, not in check)')]
perf: build
    python3 test/differential/benchmark.py --binary v1=./git-sfs

# One binary under two names must come out at ratio ~1.0. Establishes the
# measurement noise floor any regression threshold has to clear.

[group('conformance')]
[doc('perf self-check: establishes the measurement noise floor')]
perf-selfcheck: build
    python3 test/differential/benchmark.py --binary a=./git-sfs --binary b=./git-sfs
