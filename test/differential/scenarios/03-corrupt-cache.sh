#!/usr/bin/env bash
# Integrity handling: corrupt a cache object in place and confirm the tool
# reports it without repairing or removing it. Pins the exit-status split
# between presence-only verify and --with-integrity.
set -uo pipefail

setup_repo

mkdir -p "$REPO/data"
printf 'original payload\n' > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

target="$(readlink "$REPO/data/blob.bin")"
object="$REPO/data/$target"

chmod u+w "$object"
printf 'corrupted payload\n' > "$object"
chmod a-w "$object"

# --no-check-remote is load-bearing, not tidiness. The split this scenario pins
# is presence-only success versus integrity failure on the same corrupted object.
(
  cd "$REPO"
  record verify_presence git_sfs verify --no-check-remote data
  record verify_integrity git_sfs verify --no-check-remote --with-integrity data
)
