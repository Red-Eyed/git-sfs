#!/usr/bin/env bash
# A remote that is reachable but refusing: every lsjson returns 403. A local
# directory can never produce this, which is why the fake exists.
#
# contract-spec 13.3 records what v1 does here -- HasFile/CheckFile/FileSize
# return "absent" on ANY rclone error, so a 403 is indistinguishable from a
# missing object and the user is told their data is not on the remote. This
# scenario captures that baseline so v2's correction shows up as an enumerated
# divergence rather than an unexplained diff.
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

# Deny only per-object queries. The connectivity preflight still succeeds, so
# this reaches HasFile/FileSize -- where contract-spec 13.3 says any rclone
# error is reported as "the object is not on the remote".
inject_fault '{"subcommand": "lsjson", "contains": "/files/sha256/", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  record status_object_denied git_sfs status --remote default
  record verify_object_denied git_sfs verify --check-remote data
)

# Deny everything, including the preflight. v1 reports this one honestly, so the
# pair documents the boundary between the two behaviors.
inject_fault '{"subcommand": "lsd", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  record status_backend_denied git_sfs status --remote default
)
