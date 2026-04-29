set -eu

version="${1:-snapshot}"
out="${2:-dist}"
go_bin="${GO:-go}"

rm -rf "$out"
mkdir -p "$out"

for target in linux/amd64 linux/arm64 darwin/amd64 darwin/arm64; do
  os="${target%/*}"
  arch="${target#*/}"
  name="git-sfs-$version-$os-$arch"
  mkdir -p "$out/$name"
  env GOOS="$os" GOARCH="$arch" CGO_ENABLED=0 "$go_bin" build -trimpath -ldflags="-s -w" -o "$out/$name/git-sfs" ./cmd/git-sfs
  tar -C "$out/$name" -czf "$out/$name.tar.gz" git-sfs
  rm -rf "$out/$name"
done
