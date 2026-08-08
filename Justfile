# Recipes are split by lifetime, not by size. `just/go.just` is scaffolding for
# the implementation being replaced; `just/conformance.just` is the harness that
# decides whether the replacement is acceptable and outlives it.
#
# `import` rather than `mod`: modules namespace recipes but cannot be depended
# on, and `check` has to depend on all of them.

import 'just/go.just'
import 'just/rust.just'
import 'just/conformance.just'

repo := justfile_directory()
go := env_var_or_default("GO", "go")
gocache := env_var_or_default("GOCACHE", repo / ".cache/go-build")
gomodcache := env_var_or_default("GOMODCACHE", repo / ".cache/go-mod")

[doc('list recipes')]
default:
    just --list

[group('repo')]
[doc('everything CI runs: format, tests, workflows, full conformance harness')]
check: fmt test build rust-check workflows differential lock-contention cancellation mode-preservation downgrade spec-coverage
    git --no-pager diff --check

[group('repo')]
[doc('regenerate llms.txt from docs/')]
gen-llms-txt:
    uv run scripts/gen_llms_txt.py

[group('repo')]
[doc('remove build artifacts')]
clean:
    rm -rf dist git-sfs coverage.out
