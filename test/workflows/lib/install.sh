#!/usr/bin/env bash

_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  else
    shasum -a 256 "$@"
  fi
}

# Resolves the binary the suite exercises. GIT_SFS_BIN lets any implementation
# be put under test; without it the Go tree is built exactly as before. This is
# the only place that knows how a git-sfs binary comes into existence.
resolve_source_binary() {
  local built="$WORK/source-bin/git-sfs"

  if [ -n "${GIT_SFS_BIN:-}" ]; then
    [ -x "$GIT_SFS_BIN" ] || fail "GIT_SFS_BIN is not executable: $GIT_SFS_BIN"
    printf '%s\n' "$GIT_SFS_BIN"
    return
  fi

  command -v go >/dev/null 2>&1 || \
    fail "go toolchain not found; set GIT_SFS_BIN to test a prebuilt binary"

  mkdir -p "$(dirname "$built")"
  env GOOS="$HOST_OS" GOARCH="$HOST_ARCH" CGO_ENABLED=0 \
    go build -trimpath -ldflags="-s -w -X git-sfs/internal/version.Version=$BUILD_VERSION" \
    -o "$built" "$ROOT/cmd/git-sfs"
  printf '%s\n' "$built"
}

# Release assets embed the version in their filename, so the fixture is named
# after whatever the binary reports rather than a constant the suite chose.
binary_version() {
  local bin="$1"
  local reported
  reported="$("$bin" --version </dev/null | tr -d '\n')"
  [ -n "$reported" ] || fail "binary reported an empty --version: $bin"
  printf '%s\n' "$reported"
}

build_release_fixture() {
  local bin="$1"
  local release_dir="$FIXTURE_ROOT/releases/download/$VERSION"
  local latest_dir="$FIXTURE_ROOT/releases/latest"
  local asset="git-sfs-$VERSION-$HOST_OS-$HOST_ARCH.tar.gz"
  local staging="$WORK/release"

  mkdir -p "$release_dir" "$latest_dir" "$staging"
  # Mirror the published release layout locally so the installer test exercises
  # the real script and URL resolution logic instead of a test-only code path.
  cp "$bin" "$staging/git-sfs"
  chmod +x "$staging/git-sfs"
  tar -C "$staging" -czf "$release_dir/$asset" git-sfs
  (cd "$release_dir" && _sha256 "$asset" > SHA256SUMS)
  : > "$latest_dir/$VERSION"
}

install_from_fixture() {
  note "install latest release from local fixture"
  # Point the installer at local file:// endpoints so the test remains offline
  # while still covering the same contract as a real release install.
  sh "$ROOT/scripts/install.sh" \
    --version latest \
    --install-dir "$INSTALL_DIR" \
    --release-base-url "file://$FIXTURE_ROOT/releases/download" \
    --release-latest-url "file://$FIXTURE_ROOT/releases/latest/$VERSION" \
    --no-install-rclone \
    >/dev/null
  assert_eq "$(git_sfs --version | tr -d '\n')" "$VERSION" "installed git-sfs version"
}
