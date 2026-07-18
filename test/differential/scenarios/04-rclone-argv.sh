#!/usr/bin/env bash
# Captures the rclone argv stream for a full push/pull round trip. This is the
# differential artifact for the remote half, which has no tree to diff, and it
# pins the frozen remote layout (spec 5) exactly.
set -uo pipefail

use_fake_rclone
setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
printf 'first payload\n' > "$REPO/data/one.bin"
printf 'second payload\n' > "$REPO/data/two.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

(
  cd "$REPO"
  require push git_sfs push
  record status_remote git_sfs status --remote default
  record verify_remote git_sfs verify --check-remote data
)

chmod -R u+w "$CACHE/files"
rm -rf "$CACHE/files"

(
  cd "$REPO"
  record pull git_sfs pull
)
