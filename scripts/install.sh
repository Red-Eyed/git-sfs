set -eu

repo="Red-Eyed/git-sfs"
version="latest"
include_prereleases="0"
install_dir="$HOME/.local/bin"
install_rclone="1"
insecure_tls="0"
ca_bundle=""
release_base_url=""
release_latest_url=""
release_list_url=""
rclone_base_url="https://downloads.rclone.org"
curl_flags="-LsSf"

usage() {
  cat <<'EOF'
usage: install.sh [options]

Options:
  --version VERSION             git-sfs release tag to install (default: latest)
  --pre                         include prerelease versions of git-sfs
  --install-dir PATH            install directory (default: $HOME/.local/bin)
  --repo OWNER/REPO             GitHub repository (default: Red-Eyed/git-sfs)
  --release-base-url URL        release download base URL
  --release-latest-url URL      latest-release redirect URL
  --release-list-url URL        published-releases API URL
  --rclone-base-url URL         rclone download base URL
  --no-install-rclone           install only git-sfs
  --ca-bundle PATH              TLS CA bundle for downloads
  --insecure-tls                disable TLS certificate verification
  -h, --help                    show this help
EOF
}

need_value() {
  if [ "$#" -lt 2 ]; then
    echo "$1 requires a value" >&2
    usage >&2
    exit 2
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) need_value "$@"; version="$2"; shift 2 ;;
    --version=*) version="${1#*=}"; shift ;;
    --pre) include_prereleases="1"; shift ;;
    --install-dir) need_value "$@"; install_dir="$2"; shift 2 ;;
    --install-dir=*) install_dir="${1#*=}"; shift ;;
    --repo) need_value "$@"; repo="$2"; shift 2 ;;
    --repo=*) repo="${1#*=}"; shift ;;
    --release-base-url) need_value "$@"; release_base_url="$2"; shift 2 ;;
    --release-base-url=*) release_base_url="${1#*=}"; shift ;;
    --release-latest-url) need_value "$@"; release_latest_url="$2"; shift 2 ;;
    --release-latest-url=*) release_latest_url="${1#*=}"; shift ;;
    --release-list-url) need_value "$@"; release_list_url="$2"; shift 2 ;;
    --release-list-url=*) release_list_url="${1#*=}"; shift ;;
    --rclone-base-url) need_value "$@"; rclone_base_url="$2"; shift 2 ;;
    --rclone-base-url=*) rclone_base_url="${1#*=}"; shift ;;
    --no-install-rclone) install_rclone="0"; shift ;;
    --ca-bundle) need_value "$@"; ca_bundle="$2"; shift 2 ;;
    --ca-bundle=*) ca_bundle="${1#*=}"; shift ;;
    --insecure-tls) insecure_tls="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$release_base_url" ]; then
  release_base_url="https://github.com/$repo/releases/download"
fi
if [ -z "$release_latest_url" ]; then
  release_latest_url="https://github.com/$repo/releases/latest"
fi
if [ -z "$release_list_url" ]; then
  release_list_url="https://api.github.com/repos/$repo/releases?per_page=1"
fi

if [ -n "$ca_bundle" ]; then
  echo "using TLS CA bundle from $ca_bundle"
elif [ "$insecure_tls" = "1" ]; then
  curl_flags="-kLsSf"
  echo "warning: --insecure-tls disables TLS certificate verification for downloads" >&2
fi

download() {
  if [ -n "$ca_bundle" ]; then
    curl $curl_flags --cacert "$ca_bundle" "$@"
  else
    curl $curl_flags "$@"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  else
    shasum -a 256 "$@"
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

if [ "$version" = "latest" ] && [ "$include_prereleases" = "1" ]; then
  release_list="$(download "$release_list_url")"
  case "$release_list" in
    *'"tag_name"'*) ;;
    *) echo "published release list has no tag_name" >&2; exit 1 ;;
  esac
  release_list="${release_list#*\"tag_name\"}"
  release_list="${release_list#*:}"
  release_list="${release_list#*\"}"
  version="${release_list%%\"*}"
elif [ "$version" = "latest" ]; then
  latest_url="$(download -o /dev/null -w '%{url_effective}' "$release_latest_url")"
  version="${latest_url##*/}"
fi

asset="git-sfs-$version-$os-$arch.tar.gz"
url="$release_base_url/$version/$asset"
tmp_parent="$install_dir/.git-sfs-install-tmp"
tmp="$tmp_parent/git-sfs-install-$$"
rclone_os="$os"

if [ "$rclone_os" = "darwin" ]; then
  rclone_os="osx"
fi

rm -rf "$tmp"
mkdir -p "$install_dir" "$tmp"
trap 'rm -rf "$tmp"; rmdir "$tmp_parent" 2>/dev/null || true' EXIT

download "$release_base_url/$version/SHA256SUMS" -o "$tmp/SHA256SUMS"
download "$url" -o "$tmp/$asset"
expected="$(grep "  $asset$" "$tmp/SHA256SUMS" | awk '{print $1}')"
if [ -z "$expected" ]; then
  echo "SHA256SUMS has no entry for $asset" >&2
  exit 1
fi
actual="$(sha256 "$tmp/$asset" | awk '{print $1}')"
if [ "$expected" != "$actual" ]; then
  echo "SHA256 mismatch for $asset: expected $expected, got $actual" >&2
  exit 1
fi
tar -xzf "$tmp/$asset" -C "$tmp"
install "$tmp/git-sfs" "$tmp/git-sfs.install"
mv -f "$tmp/git-sfs.install" "$install_dir/git-sfs"

git_sfs_version="$("$install_dir/git-sfs" --version)"
echo "git-sfs $git_sfs_version installed to $install_dir/git-sfs"

if [ "$install_rclone" != "0" ]; then
  if ! command -v unzip >/dev/null 2>&1; then
    echo "rclone installation requires unzip; install unzip or rerun with --no-install-rclone" >&2
    exit 1
  fi
  download "$rclone_base_url/version.txt" -o "$tmp/rclone-version.txt"
  rclone_version="$(awk 'NF {print $NF; exit}' "$tmp/rclone-version.txt")"
  if [ -z "$rclone_version" ]; then
    echo "failed to determine latest stable rclone version" >&2
    exit 1
  fi
  rclone_zip="rclone-$rclone_version-$rclone_os-$arch.zip"
  rclone_url="$rclone_base_url/$rclone_version/$rclone_zip"
  download "$rclone_url" -o "$tmp/$rclone_zip"
  download "$rclone_url.sha256" -o "$tmp/$rclone_zip.sha256"
  rclone_expected="$(awk '{print $1}' "$tmp/$rclone_zip.sha256")"
  rclone_actual="$(sha256 "$tmp/$rclone_zip" | awk '{print $1}')"
  if [ "$rclone_expected" != "$rclone_actual" ]; then
    echo "SHA256 mismatch for $rclone_zip" >&2
    exit 1
  fi
  unzip -q "$tmp/$rclone_zip" -d "$tmp"
  install "$tmp"/rclone-*-*/rclone "$tmp/rclone.install"
  mv -f "$tmp/rclone.install" "$install_dir/rclone"
  rclone_installed_version="$("$install_dir/rclone" --version | awk 'NR==1 {print $2; exit}')"
  echo "rclone $rclone_installed_version installed to $install_dir/rclone"
fi
