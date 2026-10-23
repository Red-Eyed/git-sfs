#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
repo=$(pwd)

if [ "$(uname -s)" != Darwin ]; then
  echo 'Building all wheels requires a macOS host with the Apple SDK.' >&2
  exit 1
fi

mkdir -p target
work=$(mktemp -d "$repo/target/python-wheels.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for target in darwin/arm64 darwin/amd64 linux/arm64 linux/amd64; do
  sh scripts/build-python-wheel.sh "$work/dist" "$target"
done

# Preserve the previous output until every target has built successfully.
if [ -e dist ]; then
  mv dist "$work/previous-dist"
fi
if ! mv "$work/dist" dist; then
  if [ -e "$work/previous-dist" ]; then
    mv "$work/previous-dist" dist
  fi
  exit 1
fi

printf 'Built all four wheels in %s/dist/\n' "$repo"
