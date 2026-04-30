set -eu

repo="${GIT_SFS_REPO:-Red-Eyed/git-sfs}"
version="${GIT_SFS_VERSION:-latest}"
install_dir="${GIT_SFS_INSTALL_DIR:-$HOME/.local/bin}"
install_rclone="${GIT_SFS_INSTALL_RCLONE:-1}"
insecure_tls="${GIT_SFS_INSECURE_TLS:-0}"
ca_bundle="${GIT_SFS_SSL_CERT_FILE:-${SSL_CERT_FILE:-${CURL_CA_BUNDLE:-}}}"
curl_flags="-LsSf"

if [ -n "$ca_bundle" ]; then
  echo "using TLS CA bundle from $ca_bundle"
elif [ "$insecure_tls" = "1" ]; then
  curl_flags="-kLsSf"
  echo "warning: GIT_SFS_INSECURE_TLS=1 disables TLS certificate verification for downloads" >&2
fi

download() {
  if [ -n "$ca_bundle" ]; then
    curl $curl_flags --cacert "$ca_bundle" "$@"
  else
    curl $curl_flags "$@"
  fi
}

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
  latest_url="$(download -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
  version="${latest_url##*/}"
fi

asset="git-sfs-$version-$os-$arch.tar.gz"
url="https://github.com/$repo/releases/download/$version/$asset"
tmp="${TMPDIR:-/tmp}/git-sfs-install-$$"
rclone_os="$os"

if [ "$rclone_os" = "darwin" ]; then
  rclone_os="osx"
fi

rm -rf "$tmp"
mkdir -p "$tmp" "$install_dir"
trap 'rm -rf "$tmp"' EXIT

download "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
install "$tmp/git-sfs" "$install_dir/git-sfs"

echo "git-sfs installed to $install_dir/git-sfs"

if [ "$install_rclone" != "0" ]; then
  if command -v rclone >/dev/null 2>&1; then
    echo "rclone already available at $(command -v rclone)"
  else
    if ! command -v unzip >/dev/null 2>&1; then
      echo "rclone installation requires unzip; install unzip or rerun with GIT_SFS_INSTALL_RCLONE=0" >&2
      exit 1
    fi
    rclone_zip="rclone-current-$rclone_os-$arch.zip"
    rclone_url="https://downloads.rclone.org/$rclone_zip"
    download "$rclone_url" -o "$tmp/$rclone_zip"
    unzip -q "$tmp/$rclone_zip" -d "$tmp"
    install "$tmp"/rclone-*-*/rclone "$install_dir/rclone"
    echo "rclone installed to $install_dir/rclone"
  fi
fi
