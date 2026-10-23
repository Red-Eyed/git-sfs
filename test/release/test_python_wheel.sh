#!/bin/sh
# Exercise uv's real tool installation path with both bundled executables.
set -eu

wheel=$1
expected_version=$2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
export UV_TOOL_DIR="$work/tools"
export UV_TOOL_BIN_DIR="$work/bin"
export PATH="$UV_TOOL_BIN_DIR:$PATH"

uv tool install --no-index --python python3 "$wheel"
test -x "$work/bin/git-sfs"
test -x "$work/bin/rclone"
test "$(git sfs --version)" = "$expected_version"
rclone version | head -1 | grep -Fx 'rclone v1.75.2'

printf 'wheel transfer smoke test\n' > "$work/source"
rclone copyto "$work/source" "$work/destination"
cmp "$work/source" "$work/destination"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/COPYING"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/GO-LICENSE"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/GPL-3.0.txt"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/rclone-v1.75.2.tar.gz"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/rclone-v1.75.2-vendor.tar.gz"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/vendor/golang.org/x/crypto/LICENSE"
test -s "$work/tools/git-sfs/share/licenses/git-sfs/rclone/vendor/github.com/cloudsoda/sddl/LICENSE"

mkdir "$work/repo" "$work/remote"
git -C "$work/repo" init -q
(
  cd "$work/repo"
  git sfs init --cache "$work/cache"
  git sfs setup
  printf '[local]\ntype = local\n' > "$work/rclone.conf"
  cat > .git-sfs/config.toml <<EOF
version = 1
[remotes.default]
backend = "local"
path = "$work/remote"
config = "$work/rclone.conf"
EOF
  cp "$work/source" data
  git sfs add data
  git add .git-sfs/config.toml data
  git sfs push
  git sfs cache rm data
  test ! -e data
  git sfs pull --verify
  cmp data "$work/source"
)
