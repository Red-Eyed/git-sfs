#!/usr/bin/env bash
# Prepares a repo whose object is committed, cached, and already on the remote.
# Shared by the entry points that need a replicated starting state: cancellation
# interrupts a transfer in either direction, mode_preservation corrupts the local
# copy and leans on the remote to repair it.
# Sourced with lib.sh already loaded, exactly like a scenario.
set -uo pipefail

use_fake_rclone
setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
# Large enough that half of it is unmistakably partial, small enough to stay
# fast. The stall fault splits at the midpoint, so the exact size is not
# load-bearing -- only that a truncated copy cannot hash to the whole.
head -c 262144 /dev/urandom > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

(
  cd "$REPO"
  require push git_sfs push
)
