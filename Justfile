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

check: fmt test build workflows differential
    git --no-pager diff --check

release-snapshot:
    env GO={{go}} GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} sh scripts/build-release.sh snapshot dist

gen-llms-txt:
    uv run scripts/gen_llms_txt.py

clean:
    rm -rf dist git-sfs coverage.out
