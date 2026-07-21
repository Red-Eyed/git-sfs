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

# Deny only the object listing. The connectivity preflight issues `lsd local:`
# and a bare `lsjson` on the remote root, neither of which is recursive, so
# matching on --recursive lets the preflight pass and denies exactly the query
# that enumerates objects (FileSizes, command.go:525).
#
# Targeting "/files/sha256/" instead would match nothing at all: no command
# git-sfs issues names an individual object via lsjson. An earlier version of
# this scenario did exactly that, and the fault never fired -- see README,
# "Agreement is not correctness".
inject_fault '{"subcommand": "lsjson", "contains": "--recursive", "exit": 1, "stderr": "403 Forbidden: access denied"}'

# The other collapse site: with --with-integrity, verify fetches each object via
# copyto (CheckFile, command.go:195), which also returns "absent" on any error.
inject_fault '{"subcommand": "copyto", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  # status discards the error outright (status.go:96, `sizes, _ :=`) and treats
  # every object as absent, so a denied remote is indistinguishable from an
  # empty one -- and it still exits 0.
  record status_object_denied git_sfs status --remote default
  record verify_object_denied git_sfs verify --check-remote data
  record verify_integrity_denied git_sfs verify --check-remote --with-integrity data
)

# Deny everything, including the preflight. v1 reports this one honestly, so the
# pair documents the boundary between the two behaviors.
inject_fault '{"subcommand": "lsd", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  record status_backend_denied git_sfs status --remote default
)
