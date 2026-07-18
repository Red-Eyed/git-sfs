#!/usr/bin/env bash
# Replication round trip: push to a remote, drop the local object, pull it back.
# Pins the frozen remote layout (spec 5) and cache repopulation.
set -uo pipefail

setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
printf 'replicated payload\n' > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

(
  cd "$REPO"
  require push git_sfs push
  record status_remote git_sfs status --remote default
)

# Remove every cached object, leaving the symlinks dangling, so pull has to
# restore from the remote rather than find the bytes already present.
chmod -R u+w "$CACHE/files"
rm -rf "$CACHE/files"

(
  cd "$REPO"
  record verify_missing git_sfs verify data
  record pull git_sfs pull
  record verify_restored git_sfs verify data
)
