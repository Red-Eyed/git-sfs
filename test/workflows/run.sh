#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${TMPDIR:-/tmp}/git-sfs-workflows-$$"
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
VERSION="v0.0.0-workflows"
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
export PATH="$TEST_BIN_DIR:$INSTALL_DIR:$(dirname "$(command -v go)"):$PATH"

. "$ROOT/test/workflows/lib/test_lib.sh"
. "$ROOT/test/workflows/lib/install.sh"
. "$ROOT/test/workflows/lib/repo.sh"
. "$ROOT/test/workflows/lib/scenarios.sh"

main() {
  assert_repo_root_clean
  build_release_fixture
  install_from_fixture
  scenario_filesystem_workflows
  scenario_import_workflows
  scenario_rclone_workflow
  assert_repo_root_clean
  echo "workflow suite ok"
}

main "$@"
