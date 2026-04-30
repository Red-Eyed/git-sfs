#!/bin/sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/git-sfs-e2e-$$"
BIN="$WORK/git-sfs"
TEST_BIN="$WORK/bin"

run_git_sfs() {
  "$BIN" --quiet "$@" </dev/null
}

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK"
export GOCACHE="${GOCACHE:-$WORK/gocache}"
export GOMODCACHE="${GOMODCACHE:-$WORK/gomodcache}"
export GIT_TERMINAL_PROMPT=0
go build -o "$BIN" "$ROOT/cmd/git-sfs"

ensure_rclone() {
  if command -v rclone >/dev/null 2>&1; then
    return
  fi
  mkdir -p "$TEST_BIN"
  cat > "$TEST_BIN/rclone" <<'EOF'
#!/bin/sh
set -eu
if [ "${1:-}" = "--version" ]; then
  echo "rclone v0.0.0-test"
  exit 0
fi
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
cmd="$1"
src="$2"
dst="$3"
map_path() {
  case "$1" in
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
  chmod +x "$TEST_BIN/rclone"
  PATH="$TEST_BIN:$PATH"
  export PATH
}

# Build a real git repo with a real `.git` directory and repo-local cache link.
make_repo() {
  repo="$1"
  cache="$2"
  remote="$3"
  mkdir -p "$repo/data"
  git -C "$repo" init -q
  (cd "$repo" && GIT_SFS_CACHE="$cache" run_git_sfs init >/dev/null)
  cat > "$repo/.git-sfs/config.toml" <<EOF
version = 1

[remotes.default]
type = "filesystem"
url = "$remote"

[settings]
algorithm = "sha256"
EOF
}

# Exercise the plain filesystem remote against a normal Git repository.
filesystem_roundtrip() {
  repo_a="$WORK/repo-a"
  repo_b="$WORK/repo-b"
  cache_a="$WORK/cache-a"
  cache_b="$WORK/cache-b"
  remote="$WORK/remote"
  mkdir -p "$repo_a" "$repo_b" "$cache_a" "$cache_b" "$remote"
  make_repo "$repo_a" "$cache_a" "$remote"
  make_repo "$repo_b" "$cache_b" "$remote"

  printf "large payload" > "$repo_a/data/blob.bin"
  (cd "$repo_a" && run_git_sfs setup && run_git_sfs add data && run_git_sfs verify && run_git_sfs push)

  cp "$repo_a/.git-sfs/config.toml" "$repo_b/.git-sfs/config.toml"
  cp -P "$repo_a/data/blob.bin" "$repo_b/data/blob.bin"
  (cd "$repo_b" && run_git_sfs setup && run_git_sfs pull data/blob.bin && run_git_sfs verify)

  test "$(cat "$repo_b/data/blob.bin")" = "large payload"
}

# Exercise the real rclone CLI using its local backend so the test stays
# realistic without depending on any external cloud service.
rclone_roundtrip() {
  ensure_rclone

  repo="$WORK/rclone-repo"
  clone="$WORK/rclone-clone"
  remote="$WORK/rclone-remote"
  cache="$WORK/rclone-cache"
  clone_cache="$WORK/rclone-clone-cache"
  config="$WORK/rclone.conf"
  mkdir -p "$repo/data/nested" "$clone" "$remote" "$cache" "$clone_cache"

  cat > "$config" <<EOF
[local]
type = local
EOF

  git -C "$repo" init -q
  git -C "$repo" config user.email git-sfs@example.com
  git -C "$repo" config user.name git-sfs

  (cd "$repo" && GIT_SFS_CACHE="$cache" run_git_sfs init >/dev/null)
  cat > "$repo/.git-sfs/config.toml" <<EOF
version = 1

[remotes.default]
type = "rclone"
host = "local"
path = "$remote"
config = "$config"

[settings]
algorithm = "sha256"
EOF
  printf "one" > "$repo/data/one.bin"
  printf "two" > "$repo/data/nested/two.bin"

  (
    cd "$repo"
    git add .git-sfs/config.toml .gitignore
    git commit -m "initialize git-sfs" >/dev/null
    run_git_sfs add data
    git add data
    git commit -m "track dataset" >/dev/null
    run_git_sfs push
  )

  git clone "$repo" "$clone" >/dev/null
  cp "$repo/.git-sfs/config.toml" "$clone/.git-sfs/config.toml"
  ln -s "$clone_cache" "$clone/.git-sfs/cache"

  (
    cd "$clone"
    run_git_sfs setup
    run_git_sfs pull data/
    run_git_sfs verify
  )

  test "$(cat "$clone/data/one.bin")" = "one"
  test "$(cat "$clone/data/nested/two.bin")" = "two"
}

filesystem_roundtrip
rclone_roundtrip

echo "e2e ok"
