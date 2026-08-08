#!/usr/bin/env bash
# A cache object left writable, with its bytes intact.
#
# A writable cache object is suspicious but not automatically corrupt. The bytes
# are still authoritative: verification should hash the object, protect it again,
# and succeed when the digest matches.
#
# Distinct from 03-corrupt-cache, which rewrites the content. Here the content
# is untouched, so any implementation that actually reads the bytes finds
# nothing wrong -- which is precisely what makes the exit status diverge.
set -uo pipefail

setup_repo

mkdir -p "$REPO/data"
printf 'intact payload\n' > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

target="$(readlink "$REPO/data/blob.bin")"
object="$REPO/data/$target"

# Write bits restored, content left alone. This mimics a cache copied through a
# transport that preserved bytes but not permission bits.
chmod u+w "$object"

# --no-check-remote for the reason spelled out in 03-corrupt-cache: the default
# remote check is outside the cache-mode behavior being tested here.
(
  cd "$REPO"
  record verify_presence git_sfs verify --no-check-remote data
  record verify_integrity git_sfs verify --no-check-remote --with-integrity data
)
