#!/usr/bin/env bash

_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  else
    shasum -a 256 "$@"
  fi
}

# Resolves the binary the suite exercises. GIT_SFS_BIN lets a prebuilt artifact
# be put under test; without it the Rust binary is built from this tree.
resolve_source_binary() {
  local built="$WORK/source-bin/git-sfs"

  if [ -n "${GIT_SFS_BIN:-}" ]; then
    [ -x "$GIT_SFS_BIN" ] || fail "GIT_SFS_BIN is not executable: $GIT_SFS_BIN"
    printf '%s\n' "$GIT_SFS_BIN"
    return
  fi

  command -v "${CARGO:-cargo}" >/dev/null 2>&1 || \
    fail "cargo not found; set GIT_SFS_BIN to test a prebuilt binary"

  mkdir -p "$(dirname "$built")"
  (
    cd "$ROOT"
    env GIT_SFS_VERSION="$BUILD_VERSION" "${CARGO:-cargo}" build --release -p git-sfs
  )
  cp "$ROOT/target/release/git-sfs" "$built"
  chmod +x "$built"
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
  local release_list="$FIXTURE_ROOT/releases/list.json"
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
  printf '[{"tag_name":"%s","draft":false,"prerelease":true}]\n' "$VERSION" > "$release_list"
}

install_prerelease_from_fixture() {
  note "install prerelease from local fixture"
  sh "$ROOT/scripts/install.sh" \
    --pre \
    --install-dir "$INSTALL_DIR" \
    --release-base-url "file://$FIXTURE_ROOT/releases/download" \
    --release-list-url "file://$FIXTURE_ROOT/releases/list.json" \
    --no-install-rclone \
    >/dev/null
  assert_eq "$(git_sfs --version | tr -d '\n')" "$VERSION" "installed prerelease version"
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
