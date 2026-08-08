# `import` rather than `mod`: modules namespace recipes but cannot be depended
# on, and `check` has to depend on recipes split across files.
import 'just/rust.just'
import 'just/conformance.just'

repo := justfile_directory()

[doc('list recipes')]
default:
    just --list

[group('repo')]
[doc('everything CI runs: format, tests, workflows, full conformance harness')]
check: rust-check workflows differential lock-contention cancellation mode-preservation downgrade spec-coverage
    git --no-pager diff --check

[group('repo')]
[doc('regenerate llms.txt from docs/')]
gen-llms-txt:
    uv run scripts/gen_llms_txt.py

[group('repo')]
[doc('remove build artifacts')]
clean:
    rm -rf dist git-sfs coverage.out
