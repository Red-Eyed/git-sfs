#!/usr/bin/env bash
# Prepares a repo with pushable content for the lock-contention harness.
# Sourced with lib.sh already loaded, exactly like a scenario.
set -uo pipefail

use_fake_rclone
setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
printf 'lock contention payload\n' > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"
