#!/usr/bin/env sh
# Downloads and installs the rclone binary to ~/.local/bin.
# Reads __RCLONE_VERSION from the environment (e.g. vX.Y.Z).
# Detects OS and arch from uname; supports Linux/macOS x86_64/arm64.
set -eu

: "${__RCLONE_VERSION:?__RCLONE_VERSION must be set}"

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)

case "$os" in
  linux) ;;
  darwin) os="osx" ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac

case "$arch" in
  x86_64)  arch="amd64" ;;
  aarch64|arm64) arch="arm64" ;;
  *) echo "unsupported arch: $arch" >&2; exit 1 ;;
esac

url="https://downloads.rclone.org/${__RCLONE_VERSION}/rclone-${__RCLONE_VERSION}-${os}-${arch}.zip"

mkdir -p "$HOME/.local/bin"
curl -fsSL "$url" -o rclone.zip
unzip -j rclone.zip "*/rclone" -d "$HOME/.local/bin"
rm rclone.zip
chmod +x "$HOME/.local/bin/rclone"
