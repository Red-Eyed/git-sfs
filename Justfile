go := env_var_or_default("GO", "go")
gocache := env_var_or_default("GOCACHE", "/private/tmp/git-sfs-go-cache")
gomodcache := env_var_or_default("GOMODCACHE", "/private/tmp/git-sfs-go-modcache")

default:
    just --list

fmt:
    {{go}}fmt -w cmd internal

test:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test ./...

coverage:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test -covermode=atomic -coverprofile=coverage.out ./...
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} tool cover -func=coverage.out

bench:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test -run '^$' -bench . -benchmem ./...

build:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} build ./cmd/git-sfs

workflows:
    env PATH="$(dirname {{go}}):$PATH" GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} bash test/workflows/run.sh

# Self-check: the same binary compared against itself must always agree, which
# guards the harness against nondeterminism. Pass two --binary flags to run.py
# directly to compare different implementations.
differential: build
    python3 test/differential/run.py --binary a=./git-sfs --binary b=./git-sfs

# Lock protocol conformance. Pass a second --binary to run.py's sibling to
# exercise real cross-version contention.
lock-contention: build
    python3 test/differential/lock_contention.py --binary v1=./git-sfs

# Cancellation safety: SIGINT mid-transfer must publish no partial file and must
# leave the operation retryable. Asserts invariants rather than diffing trees,
# since an interrupt lands where no binary controls.
cancellation: build
    python3 test/differential/cancellation.py --binary v1=./git-sfs

# Mode/content disagreement (contract-spec 4.1): what happens when the read-only
# bit that stands in for "hash-verified" no longer tells the truth.
mode-preservation: build
    python3 test/differential/mode_preservation.py --binary v1=./git-sfs

# Performance baselines (rust-rewrite-plan 9b). Deliberately NOT part of `check`:
# this is the nightly/on-demand tier, and it takes minutes rather than seconds.
# Pass a second --binary to compare implementations side by side, which is the
# only form the Phase 7 gate accepts -- absolute times are machine-specific.
perf: build
    python3 test/differential/benchmark.py --binary v1=./git-sfs

# Self-check: one binary under two names must come out at ratio ~1.0. Establishes
# the measurement noise floor any regression threshold has to clear.
perf-selfcheck: build
    python3 test/differential/benchmark.py --binary a=./git-sfs --binary b=./git-sfs

check: fmt test build workflows differential lock-contention cancellation mode-preservation
    git --no-pager diff --check

release-snapshot:
    env GO={{go}} GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} sh scripts/build-release.sh snapshot dist

gen-llms-txt:
    uv run scripts/gen_llms_txt.py

clean:
    rm -rf dist git-sfs coverage.out
