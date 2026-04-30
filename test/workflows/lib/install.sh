#!/usr/bin/env bash

build_release_fixture() {
  local release_dir="$FIXTURE_ROOT/releases/download/$VERSION"
  local latest_dir="$FIXTURE_ROOT/releases/latest"
  local asset="git-sfs-$VERSION-$HOST_OS-$HOST_ARCH.tar.gz"
  local staging="$WORK/release"

  mkdir -p "$release_dir" "$latest_dir" "$staging"
  # Mirror the published release layout locally so the installer test exercises
  # the real script and URL resolution logic instead of a test-only code path.
  env GOOS="$HOST_OS" GOARCH="$HOST_ARCH" CGO_ENABLED=0 \
    go build -trimpath -ldflags="-s -w -X git-sfs/internal/version.Version=$VERSION" \
    -o "$staging/git-sfs" "$ROOT/cmd/git-sfs"
  tar -C "$staging" -czf "$release_dir/$asset" git-sfs
  : > "$latest_dir/$VERSION"
}

install_from_fixture() {
  note "install latest release from local fixture"
  # Point the installer at local file:// endpoints so the test remains offline
  # while still covering the same contract as a real release install.
  env \
    GIT_SFS_VERSION=latest \
    GIT_SFS_INSTALL_DIR="$INSTALL_DIR" \
    GIT_SFS_RELEASE_BASE_URL="file://$FIXTURE_ROOT/releases/download" \
    GIT_SFS_RELEASE_LATEST_URL="file://$FIXTURE_ROOT/releases/latest/$VERSION" \
    GIT_SFS_INSTALL_RCLONE=0 \
    sh "$ROOT/scripts/install.sh" >/dev/null
  assert_eq "$(git_sfs --version | tr -d '\n')" "$VERSION" "installed git-sfs version"
}
