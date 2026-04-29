set -eu

repo="${MERK_REPO:-Red-Eyed/merk}"
version="${MERK_VERSION:-latest}"
install_dir="${MERK_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

case "$os" in
  darwin|linux) ;;
  *) echo "unsupported os: $os" >&2; exit 1 ;;
esac

case "$arch" in
  x86_64|amd64) arch="amd64" ;;
  arm64|aarch64) arch="arm64" ;;
  *) echo "unsupported arch: $arch" >&2; exit 1 ;;
esac

if [ "$version" = "latest" ]; then
  latest_url="$(curl -LsS -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
  version="${latest_url##*/}"
fi

asset="merk-$version-$os-$arch.tar.gz"
url="https://github.com/$repo/releases/download/$version/$asset"
tmp="${TMPDIR:-/tmp}/merk-install-$$"

rm -rf "$tmp"
mkdir -p "$tmp" "$install_dir"
trap 'rm -rf "$tmp"' EXIT

curl -LsSf "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
install "$tmp/merk" "$install_dir/merk"

echo "merk installed to $install_dir/merk"
