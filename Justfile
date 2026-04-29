go := env_var_or_default("GO", "/Users/vadstup/.local/go/bin/go")
gocache := env_var_or_default("GOCACHE", "/private/tmp/merk-go-cache")

default:
    just --list

fmt:
    {{go}}fmt -w cmd internal

test:
    env GOCACHE={{gocache}} {{go}} test ./...

coverage:
    env GOCACHE={{gocache}} {{go}} test -covermode=atomic -coverprofile=coverage.out ./...
    env GOCACHE={{gocache}} {{go}} tool cover -func=coverage.out

build:
    env GOCACHE={{gocache}} {{go}} build ./cmd/merk

smoke:
    env PATH="$(dirname {{go}}):$PATH" GOCACHE={{gocache}} bash scripts/smoke.sh

check: fmt test build smoke
    git diff --check

release-snapshot:
    env GO={{go}} GOCACHE={{gocache}} sh scripts/build-release.sh snapshot dist

clean:
    rm -rf dist merk coverage.out
