#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${TMPDIR:-/tmp}/git-sfs-workflows-$$"
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
BUILD_VERSION="v0.0.0-workflows"
INSTALL_DIR="$WORK/install/bin"
TEST_BIN_DIR="$WORK/test-bin"
FIXTURE_ROOT="$WORK/fixtures"
REMOTE_ROOT="$WORK/remotes"

case "$HOST_ARCH" in
  x86_64|amd64) HOST_ARCH="amd64" ;;
  arm64|aarch64) HOST_ARCH="arm64" ;;
  *) echo "unsupported arch: $HOST_ARCH" >&2; exit 1 ;;
esac

case "$HOST_OS" in
  darwin|linux) ;;
  *) echo "unsupported os: $HOST_OS" >&2; exit 1 ;;
esac

cleanup() {
  # Go writes module cache files read-only, so plain rm -rf fails on the temp
  # GOMODCACHE. Restore write permission first so teardown always succeeds.
  chmod -R u+w "$WORK" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

assert_repo_root_clean() {
  if [ -e "$ROOT/.git-sfs" ]; then
    echo "workflow suite must not create $ROOT/.git-sfs" >&2
    exit 1
  fi
}

mkdir -p "$INSTALL_DIR" "$TEST_BIN_DIR" "$FIXTURE_ROOT" "$REMOTE_ROOT"
export GOCACHE="${GOCACHE:-$WORK/gocache}"
export GOMODCACHE="${GOMODCACHE:-$WORK/gomodcache}"
export GIT_TERMINAL_PROMPT=0

# Go is only needed to build the binary from this tree. Running against a
# prebuilt GIT_SFS_BIN must not require a toolchain to be installed at all.
GO_BIN_DIR=""
if _go_path="$(command -v go 2>/dev/null)"; then
  GO_BIN_DIR="$(dirname "$_go_path")"
fi
export PATH="$TEST_BIN_DIR:$INSTALL_DIR${GO_BIN_DIR:+:$GO_BIN_DIR}:$PATH"

. "$ROOT/test/workflows/lib/test_lib.sh"
. "$ROOT/test/workflows/lib/install.sh"
. "$ROOT/test/workflows/lib/repo.sh"
. "$ROOT/test/workflows/lib/scenarios.sh"

require_rclone() {
  if ! command -v rclone >/dev/null 2>&1; then
    echo "rclone is required for the workflow suite" >&2
    exit 1
  fi
}

main() {
  assert_repo_root_clean
  require_rclone
  SOURCE_BIN="$(resolve_source_binary)"
  VERSION="$(binary_version "$SOURCE_BIN")"
  note "testing git-sfs $VERSION ($SOURCE_BIN)"
  build_release_fixture "$SOURCE_BIN"
  install_from_fixture
  scenario_filesystem_workflows
  scenario_import_workflows
  scenario_rclone_workflow
  assert_repo_root_clean
  echo "workflow suite ok"
}

main "$@"
