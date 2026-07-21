#!/usr/bin/env bash
# A cache object left writable, with its bytes intact.
#
# contract-spec 4.1 makes the read-only bit *mechanism* (frozen) but trusting it
# as proof of verification *policy* (v2's to change). This is the one state
# where the two readings must disagree: v1 reports wrong-cache-permissions and
# exits non-zero without reading a byte, while v2 is expected to hash-verify the
# object, find it sound, protect it in place, and succeed.
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

# Write bits restored, content left alone. Mimics the environments 4.1 lists --
# an exFAT copy, rsync without -p, an archive extraction -- where the bit is
# lost in transit while the bytes survive.
chmod u+w "$object"

# --no-check-remote for the reason spelled out in 03-corrupt-cache: the default
# remote check fails on the template's missing rclone.conf and masks the cache
# result behind a uniform exit 2. v1 answers 3 to both of these.
#
# Only verify_integrity is declared as a divergence. verify_presence is left
# comparing strictly on purpose: whether a presence-only check should hash an
# object in order to judge its mode is an open design question, and 4.1 does not
# settle it. If v2 changes this one too, the harness goes red and forces the
# decision to be made deliberately rather than inherited by accident.
(
  cd "$REPO"
  record verify_presence git_sfs verify --no-check-remote data
  record verify_integrity git_sfs verify --no-check-remote --with-integrity data
)
