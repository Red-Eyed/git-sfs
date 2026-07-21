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

# --no-check-remote is load-bearing, not tidiness. This repo has only the init
# template's config, whose `config = "rclone.conf"` names a file init never
# creates (contract-spec 13.3), so the default --check-remote makes verify exit 2
# before it looks at the cache at all. Without the flag both records below are 2
# and identical, and this scenario silently stops testing corruption -- it passed
# just as green against an intact cache. The split it exists to pin is 0 vs 3.
(
  cd "$REPO"
  record verify_presence git_sfs verify --no-check-remote data
  record verify_integrity git_sfs verify --no-check-remote --with-integrity data
)
