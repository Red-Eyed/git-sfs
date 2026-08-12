set -eu

version="${1:-snapshot}"
out="${2:-dist}"
cargo="${CARGO:-cargo}"

if [ "$#" -gt 0 ]; then
  shift
fi
if [ "$#" -gt 0 ]; then
  shift
fi

if [ "$#" -gt 0 ]; then
  targets="$*"
else
  targets="linux/amd64 linux/arm64 darwin/amd64 darwin/arm64"
fi

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  else
    shasum -a 256 "$@"
  fi
}

native_os() {
  case "$(uname -s)" in
    Darwin) printf '%s\n' darwin ;;
    Linux) printf '%s\n' linux ;;
    *) echo "unsupported native os: $(uname -s)" >&2; exit 1 ;;
  esac
}

native_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf '%s\n' amd64 ;;
    arm64|aarch64) printf '%s\n' arm64 ;;
    *) echo "unsupported native arch: $(uname -m)" >&2; exit 1 ;;
  esac
}

target_triple() {
  case "$1" in
    linux/amd64) printf '%s\n' x86_64-unknown-linux-musl ;;
    linux/arm64) printf '%s\n' aarch64-unknown-linux-musl ;;
    darwin/amd64) printf '%s\n' x86_64-apple-darwin ;;
    darwin/arm64) printf '%s\n' aarch64-apple-darwin ;;
    *) echo "unsupported release target: $1" >&2; exit 1 ;;
  esac
}

build_target() {
  target="$1"
  os="${target%/*}"
  arch="${target#*/}"

  if [ "$target" = "native" ]; then
    os="$(native_os)"
    arch="$(native_arch)"
    env GIT_SFS_VERSION="$version" "$cargo" build --release -p git-sfs
    binary="target/release/git-sfs"
  else
    triple="$(target_triple "$target")"
    env GIT_SFS_VERSION="$version" "$cargo" zigbuild --release -p git-sfs --target "$triple"
    binary="target/$triple/release/git-sfs"
  fi

  if [ "$os" = "linux" ]; then
    case "$(file "$binary")" in
      *"statically linked"*|*"static-pie linked"*) ;;
      *) echo "$binary is not statically linked" >&2; exit 1 ;;
    esac
  fi

  name="git-sfs-$version-$os-$arch"
  mkdir -p "$out/$name"
  cp "$binary" "$out/$name/git-sfs"
  chmod 755 "$out/$name/git-sfs"
  tar -C "$out/$name" -czf "$out/$name.tar.gz" git-sfs
  rm -rf "$out/$name"
}

rm -rf "$out"
mkdir -p "$out"

for target in $targets; do
  build_target "$target"
done

(cd "$out" && sha256 *.tar.gz > SHA256SUMS)
