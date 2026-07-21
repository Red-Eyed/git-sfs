#!/usr/bin/env bash
# Prepares an initialized repo with a working remote and no payload -- the
# benchmark generates its own fixtures, since their size and count are the
# variables under study.
# Sourced with lib.sh already loaded, exactly like a scenario.
set -uo pipefail

use_fake_rclone
setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
