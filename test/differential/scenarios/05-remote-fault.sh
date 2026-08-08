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

# Deny only the object listing. The connectivity preflight issues `lsd local:`
# and a bare `lsjson` on the remote root, neither of which is recursive, so
# matching on --recursive lets the preflight pass and denies exactly the query
# that enumerates objects.
#
# Targeting "/files/sha256/" instead would match nothing at all: no command
# git-sfs issues names an individual object via lsjson. An earlier version of
# this scenario did exactly that, and the fault never fired -- see README,
# "Agreement is not correctness".
inject_fault '{"subcommand": "lsjson", "contains": "--recursive", "exit": 1, "stderr": "403 Forbidden: access denied"}'

# The other collapse site: with --with-integrity, verify fetches each object via
# copyto, which also returns "absent" on any error.
inject_fault '{"subcommand": "copyto", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  # A denied object listing should surface as unknown remote state, not as an
  # empty remote.
  record status_object_denied git_sfs status --remote default
  record verify_object_denied git_sfs verify --check-remote data
  record verify_integrity_denied git_sfs verify --check-remote --with-integrity data
)

# Deny everything, including the preflight. This pins the boundary between an
# object-level lookup failure and a backend-level connectivity failure.
inject_fault '{"subcommand": "lsd", "exit": 1, "stderr": "403 Forbidden: access denied"}'

(
  cd "$REPO"
  record status_backend_denied git_sfs status --remote default
)
