# Go toolchain. Everything here disappears at the Rust cutover (rust-rewrite-plan
# phase 7), which is why it is a separate file rather than mixed into the root.

[group('go')]
[doc('gofmt the tree')]
fmt:
    {{go}}fmt -w cmd internal

[group('go')]
[doc('Go unit and workflow integration tests')]
test:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test ./...

[group('go')]
[doc('Go test coverage report')]
coverage:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test -covermode=atomic -coverprofile=coverage.out ./...
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} tool cover -func=coverage.out

[group('go')]
[doc('Go microbenchmarks')]
bench:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} test -run '^$' -bench . -benchmem ./...

[group('go')]
[doc('build ./git-sfs')]
build:
    env GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} {{go}} build ./cmd/git-sfs

[group('go')]
[doc('build release archives into dist/')]
release-snapshot:
    env GO={{go}} GOCACHE={{gocache}} GOMODCACHE={{gomodcache}} sh scripts/build-release.sh snapshot dist
