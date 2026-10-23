#!/bin/sh
# Release wheels must be built here so rclone and its notices accompany git-sfs.
set -eu

cd "$(dirname "$0")/.."
repo=$(pwd)
out=${1:-dist}
target=${2:-native}

if [ "$target" = native ]; then
  case "$(uname -s)" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) echo 'unsupported operating system' >&2; exit 1 ;;
  esac
  case "$(uname -m)" in
    x86_64) arch=amd64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) echo 'unsupported architecture' >&2; exit 1 ;;
  esac
  target=$os/$arch
fi

case "$target" in
  linux/amd64) triple=x86_64-unknown-linux-musl; goos=linux; goarch=amd64 ;;
  linux/arm64) triple=aarch64-unknown-linux-musl; goos=linux; goarch=arm64 ;;
  darwin/amd64) triple=x86_64-apple-darwin; goos=darwin; goarch=amd64 ;;
  darwin/arm64) triple=aarch64-apple-darwin; goos=darwin; goarch=arm64 ;;
  *) echo "unsupported wheel target: $target" >&2; exit 1 ;;
esac

rustup target add "$triple"
mkdir -p target
work=$(mktemp -d "$repo/target/python-wheel.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

fetch() {
  url=$1
  expected=$2
  destination=$3
  curl --fail --location --silent --show-error "$url" -o "$destination"
  actual=$(shasum -a 256 "$destination")
  actual=${actual%% *}
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 mismatch for $url" >&2
    exit 1
  fi
}

rclone_version=v1.75.2
base=https://downloads.rclone.org/$rclone_version
fetch "$base/rclone-$rclone_version.tar.gz" \
  fa64537e7276196310dbb0bcea6330f03b40e5790197b88f14cce6a19f40eb67 "$work/source.tar.gz"
fetch "$base/rclone-$rclone_version-vendor.tar.gz" \
  69b356c0b1800052c33ce0e0e11f18df8717a721aab346b553dc1ab44c8caae8 "$work/vendor.tar.gz"
tar -xzf "$work/source.tar.gz" -C "$work"
source_dir=$work/rclone-$rclone_version
tar -xzf "$work/vendor.tar.gz" -C "$source_dir"

# A shared staging path is required by Maturin's data setting. Targets are built
# sequentially, and only a complete rclone build replaces the staged executable.
data=$repo/target/python-wheel-data
mkdir -p "$data/scripts"
export GOCACHE="$repo/target/go-build-cache"
export GOPATH="$repo/target/go"
(
  cd "$source_dir"
  env CGO_ENABLED=0 GOOS="$goos" GOARCH="$goarch" go build \
    -mod=vendor -trimpath -buildvcs=false \
    -ldflags="-s -w -X github.com/rclone/rclone/fs.Version=$rclone_version" \
    -o "$work/rclone" .
)
cp "$work/rclone" "$data/scripts/rclone"
chmod 755 "$data/scripts/rclone"

notices=$data/data/share/licenses/git-sfs/rclone
mkdir -p "$notices"
cp "$source_dir/COPYING" "$notices/COPYING"
cp "$source_dir/go.mod" "$notices/go.mod"
goroot=$(go env GOROOT)
cp "$goroot/LICENSE" "$notices/GO-LICENSE"
fetch https://raw.githubusercontent.com/spdx/license-list-data/v3.27.0/text/GPL-3.0-only.txt \
  fb981668c18a279e285fc4d83fba1e836cc84dd4daa73c9697d3cfd2d8aca6e0 "$notices/GPL-3.0.txt"
# The statically linked SMB dependency is LGPLv3. Include corresponding
# application and dependency sources so recipients can modify and relink it.
cp "$work/source.tar.gz" "$notices/rclone-$rclone_version.tar.gz"
cp "$work/vendor.tar.gz" "$notices/rclone-$rclone_version-vendor.tar.gz"
(
  cd "$source_dir"
  find vendor -type f \( -iname '*license*' -o -iname '*licence*' \
    -o -iname '*copying*' -o -iname '*copyright*' -o -iname '*notice*' \) \
    > "$work/notices"
  tar -cf "$work/notices.tar" -T "$work/notices"
)
tar -xf "$work/notices.tar" -C "$notices"
cat > "$notices/README.txt" <<EOF
This wheel includes rclone $rclone_version, built without cgo or optional cmount.
Source: $base/rclone-$rclone_version.tar.gz
Dependencies: $base/rclone-$rclone_version-vendor.tar.gz
The accompanying vendor directory preserves upstream license and notice files.
These include github.com/cloudsoda/sddl under LGPLv3 and MPL-covered libraries.
The complete rclone and vendored dependency source archives are included here.
The Go runtime is covered by GO-LICENSE.
Go source: https://go.dev/dl/$(go env GOVERSION).src.tar.gz

To rebuild rclone, extract these archives into a writable directory:
  tar -xzf rclone-$rclone_version.tar.gz
  cd rclone-$rclone_version
  tar -xzf ../rclone-$rclone_version-vendor.tar.gz
Modify the library sources under vendor as desired, then run with Go 1.26:
  CGO_ENABLED=0 GOOS=$goos GOARCH=$goarch go build -mod=vendor -trimpath -buildvcs=false -ldflags='-s -w -X github.com/rclone/rclone/fs.Version=$rclone_version' -o ../rclone .
The rebuilt executable can replace rclone in the tool environment's bin directory.
EOF

pkgid=$(cargo pkgid -p git-sfs)
version=${pkgid##*#}
version=${version##*@}
export GIT_SFS_VERSION="v$version"
if [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_TYPE:-}" = tag ] \
  && [ "$GITHUB_REF_NAME" != "$GIT_SFS_VERSION" ]; then
  echo "release tag $GITHUB_REF_NAME differs from Cargo version $GIT_SFS_VERSION" >&2
  exit 1
fi

if [ "$goos" = linux ]; then
  export CARGO_ZIGBUILD_CACHE_DIR="$repo/target/zigbuild-cache"
  export ZIG_GLOBAL_CACHE_DIR="$repo/target/zig-cache"
  case "$(file "$data/scripts/rclone")" in
    *'statically linked'*) ;;
    *) echo 'rclone must be statically linked in Linux wheels' >&2; exit 1 ;;
  esac
  uv tool run --from maturin==1.15.0 --with ziglang==0.15.2 maturin build \
    --release --locked --target "$triple" --zig \
    --compatibility manylinux_2_17 musllinux_1_2 --out "$out"
else
  MACOSX_DEPLOYMENT_TARGET=12.0 uv tool run --from maturin==1.15.0 maturin build \
    --release --locked --target "$triple" --out "$out"
fi
