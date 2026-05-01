#!/usr/bin/env bash

write_filesystem_config() {
  # Callers must set RCLONE_TEST_ROOT to the remote directory and install the
  # fake rclone binary before calling this.  Files land at
  # $RCLONE_TEST_ROOT/files/sha256/...
  local repo="$1"
  cat > "$repo/.git-sfs/config.toml" <<'EOF'
version = 1

[remotes.default]
backend = "testremote"

[settings]
algorithm = "sha256"
n_jobs = 0
EOF
}

write_rclone_config() {
  local repo="$1"
  local remote="$2"
  local path="$3"
  local config="$4"
  cat > "$repo/.git-sfs/config.toml" <<EOF
version = 1

[remotes.default]
backend = "$remote"
path = "$path"
config = "$config"

[settings]
algorithm = "sha256"
n_jobs = 0
EOF
}

init_repo() {
  local repo="$1"
  local cache="$2"
  mkdir -p "$repo"
  git -C "$repo" init -q
  git_setup_user "$repo"
  (
    cd "$repo"
    # `init` establishes tracked repo state, while `setup` materializes the
    # repo-local cache symlink that stays out of Git history.
    GIT_SFS_CACHE="$cache" git_sfs init >/dev/null
    git_sfs setup >/dev/null
  )
}

install_fake_rclone() {
  # This stub only implements the tiny surface area the tests rely on. That
  # keeps the workflow realistic from git-sfs's point of view without pulling
  # in the real binary for every local run.
  cat > "$TEST_BIN_DIR/rclone" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "--version" ]; then
  echo "rclone v0.0.0-test"
  exit 0
fi
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
cmd="${1:-}"
src="${2:-}"
dst="${3:-}"
map_path() {
  # `testremote:` resolves inside the test-owned fake remote root. Any other
  # remote syntax falls back to a simple absolute path mapping for completeness.
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *:*) printf '/%s\n' "${1#*:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
src="$(map_path "$src")"
dst="$(map_path "$dst")"
case "$cmd" in
  copyto)
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst"
    ;;
  lsjson)
    if [ -e "$src" ]; then
      printf '[{"Path":"%s"}]\n' "$(basename "$src")"
    else
      printf '[]\n'
    fi
    ;;
  moveto)
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  *)
    echo "unsupported rclone command: $cmd" >&2
    exit 2
    ;;
esac
EOF
  chmod +x "$TEST_BIN_DIR/rclone"
}
