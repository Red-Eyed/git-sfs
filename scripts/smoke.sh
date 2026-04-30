set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/git-sfs-smoke-$$"
BIN="$WORK/git-sfs"
CACHE_A="$WORK/cache-a"
CACHE_B="$WORK/cache-b"
REMOTE="$WORK/remote"
REPO_A="$WORK/repo-a"
REPO_B="$WORK/repo-b"

run_git_sfs() {
  "$BIN" --quiet "$@" </dev/null
}

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK" "$REPO_A" "$REPO_B"
export GIT_TERMINAL_PROMPT=0
go build -o "$BIN" "$ROOT/cmd/git-sfs"

init_repo() {
  local repo="$1"
  local cache="$2"
  mkdir -p "$repo/.git" "$repo/.git-sfs"
  cat > "$repo/.git-sfs/config.toml" <<EOF
version = 1

[remotes.default]
type = "filesystem"
url = "$REMOTE"

[settings]
algorithm = "sha256"
EOF
  ln -s "$cache" "$repo/.git-sfs/cache"
}

init_repo "$REPO_A" "$CACHE_A"
mkdir -p "$REPO_A/data"
printf "large payload" > "$REPO_A/data/blob.bin"

(cd "$REPO_A" && run_git_sfs setup && run_git_sfs add data && run_git_sfs verify && run_git_sfs push)

mkdir -p "$REPO_B/.git" "$REPO_B/.git-sfs" "$REPO_B/data"
cp "$REPO_A/.git-sfs/config.toml" "$REPO_B/.git-sfs/config.toml"
cp -P "$REPO_A/data/blob.bin" "$REPO_B/data/blob.bin"
ln -s "$CACHE_B" "$REPO_B/.git-sfs/cache"

(cd "$REPO_B" && run_git_sfs setup && run_git_sfs pull data/blob.bin && run_git_sfs verify)
test "$(cat "$REPO_B/data/blob.bin")" = "large payload"

echo "smoke ok"
