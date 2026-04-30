#!/usr/bin/env bash

note() {
  printf '==> %s\n' "$1"
}

fail() {
  echo "workflows: $1" >&2
  exit 1
}

assert_eq() {
  local got="$1"
  local want="$2"
  local msg="$3"
  if [ "$got" != "$want" ]; then
    fail "$msg: got [$got], want [$want]"
  fi
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local msg="$3"
  case "$haystack" in
    *"$needle"*) ;;
    *) fail "$msg: missing [$needle] in [$haystack]" ;;
  esac
}

assert_symlink() {
  local path="$1"
  [ -L "$path" ] || fail "expected symlink: $path"
}

resolve_symlink_target() {
  local path="$1"
  local dir target
  dir="$(cd "$(dirname "$path")" && pwd -P)"
  target="$(readlink "$path")"
  cd "$dir" && cd "$(dirname "$target")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$target")"
}

git_sfs() {
  "$INSTALL_DIR/git-sfs" --quiet "$@" </dev/null
}

git_setup_user() {
  local repo="$1"
  git -C "$repo" config user.email git-sfs@example.com
  git -C "$repo" config user.name git-sfs
}

hash_from_link() {
  local repo="$1"
  local path="$2"
  local target
  target="$(readlink "$repo/$path")"
  basename "$target"
}

cache_file_for() {
  local cache="$1"
  local hash="$2"
  printf '%s/files/sha256/%s/%s\n' "$cache" "${hash%${hash#??}}" "$hash"
}
