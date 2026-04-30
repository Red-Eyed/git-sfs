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

build:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} build ./cmd/git-sfs

workflows:
    env PATH="$(dirname {{go}}):$PATH" GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} bash test/workflows/run.sh

check: fmt test build workflows
    git --no-pager diff --check

release-snapshot:
    env GO={{go}} GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} sh scripts/build-release.sh snapshot dist

clean:
    rm -rf dist git-sfs coverage.out
