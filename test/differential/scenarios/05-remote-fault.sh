#!/usr/bin/env bash
# A remote that is reachable but refusing: every lsjson returns 403. A local
# directory can never produce this, which is why the fake exists.
#
# A denied remote must not be reported as a clean "missing object" result. This
# scenario keeps permission and availability failures distinct.
set -uo pipefail

use_fake_rclone
setup_repo
write_local_remote_config

mkdir -p "$REPO/data"
printf 'payload\n' > "$REPO/data/blob.bin"

(
  cd "$REPO"
  require add git_sfs add data
)
commit_all "track dataset"

(
  cd "$REPO"
  require push git_sfs push
)

# Deny the one exact-path metadata batch. Matching `--files-from` distinguishes
# object-set lookup from doctor's connectivity probes without depending on any
# individual object path.
inject_fault '{"subcommand": "lsjson", "contains": "--files-from", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  # A denied object listing should surface as unknown remote state, not as an
  # empty remote.
  record status_object_denied git_sfs status --remote default
  record verify_object_denied git_sfs verify --check-remote data
  record verify_integrity_denied git_sfs verify --check-remote --with-integrity data
)

# Deny doctor's distinct backend probe as well. Object commands never issue
# this diagnostic call; they rely on their batched metadata/transfer operation.
inject_fault '{"subcommand": "lsd", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  record doctor_backend_denied git_sfs doctor
)
