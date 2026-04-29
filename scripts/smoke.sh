set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/merk-smoke-$$"
BIN="$WORK/merk"
CACHE_A="$WORK/cache-a"
CACHE_B="$WORK/cache-b"
REMOTE="$WORK/remote"
REPO_A="$WORK/repo-a"
REPO_B="$WORK/repo-b"

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK" "$REPO_A" "$REPO_B"
go build -o "$BIN" "$ROOT/cmd/merk"

init_repo() {
  local repo="$1"
  local cache="$2"
  mkdir -p "$repo/.git" "$repo/.ds"
  cat > "$repo/dataset.yaml" <<EOF
version: 1

remotes:
  default:
    type: filesystem
    url: $REMOTE

settings:
  algorithm: sha256
EOF
  cat > "$repo/.ds/local.yaml" <<EOF
cache:
  path: $cache
EOF
}

init_repo "$REPO_A" "$CACHE_A"
mkdir -p "$REPO_A/data"
printf "large payload" > "$REPO_A/data/blob.bin"

(cd "$REPO_A" && "$BIN" setup && "$BIN" add data && "$BIN" verify && "$BIN" push)

cp "$REPO_A/dataset.yaml" "$REPO_B/dataset.yaml"
mkdir -p "$REPO_B/.git" "$REPO_B/.ds" "$REPO_B/data"
cp -P "$REPO_A/data/blob.bin" "$REPO_B/data/blob.bin"
cat > "$REPO_B/.ds/local.yaml" <<EOF
cache:
  path: $CACHE_B
EOF

(cd "$REPO_B" && "$BIN" setup && "$BIN" pull data/blob.bin && "$BIN" verify)
test "$(cat "$REPO_B/data/blob.bin")" = "large payload"

echo "smoke ok"
