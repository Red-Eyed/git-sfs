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

(
  cd "$REPO"
  record verify_presence git_sfs verify data
  record verify_integrity git_sfs verify --with-integrity data
)
