#!/usr/bin/env bash
# Helpers shared by differential scenarios.
#
# Scenarios run against whichever binary is under test and must behave
# identically for all of them. They receive these variables from the driver:
#
#   GIT_SFS   binary under test
#   WORK      workspace root; everything below lives inside it
#   REPO      git repository to operate on
#   CACHE     cache root
#   REMOTE    directory backing the rclone "local" remote
#   OUTCOMES  file collecting per-command exit statuses

git_sfs() {
  "$GIT_SFS" --quiet "$@" </dev/null
}

# Runs a command that MUST succeed for the rest of the scenario to mean
# anything, recording its status and aborting if it fails.
#
# The distinction from `record` is load-bearing. A differential harness compares
# binaries against each other, so a broken fixture makes both sides fail
# identically and reads as green -- it verifies agreement, not correctness. Use
# `require` for setup whose failure would hollow out the scenario, and `record`
# only where the outcome is itself under test.
require() {
  local label="$1"
  shift
  local status=0
  "$@" >/dev/null 2>&1 || status=$?
  printf '%s=%s\n' "$label" "$status" >> "$OUTCOMES"
  if [ "$status" -ne 0 ]; then
    echo "scenario precondition failed: $label exited $status" >&2
    # A sentinel file, not just `exit`: require is normally called inside a
    # `( cd "$REPO"; ... )` subshell, where exit ends only the subshell and the
    # scenario would carry on as though the precondition had held.
    printf '%s exited %s\n' "$label" "$status" >> "$WORK/precondition-failed"
    exit "$status"
  fi
}

# Runs a command, recording its exit status instead of aborting the scenario.
# Error paths are part of what the harness compares, so a failing command has to
# leave the run alive -- otherwise the tree after a failure is never captured.
record() {
  local label="$1"
  shift
  local status=0
  "$@" >/dev/null 2>&1 || status=$?
  printf '%s=%s\n' "$label" "$status" >> "$OUTCOMES"
}

setup_repo() {
  mkdir -p "$REPO"
  git -C "$REPO" init -q
  git -C "$REPO" config user.email git-sfs@example.com
  git -C "$REPO" config user.name git-sfs
  (
    cd "$REPO"
    git_sfs init --cache "$CACHE" >/dev/null
    git_sfs setup >/dev/null
  )
}

write_local_remote_config() {
  local rclone_cfg="$WORK/rclone.conf"
  printf '[local]\ntype = local\n' > "$rclone_cfg"
  cat > "$REPO/.git-sfs/config.toml" <<EOF
version = 1

[remotes.default]
backend = "local"
path = "$REMOTE"
config = "$rclone_cfg"

[settings]
algorithm = "sha256"
n_jobs = 0
EOF
}

commit_all() {
  git -C "$REPO" add -A
  git -C "$REPO" commit -qm "$1"
}

# Puts the recording fake rclone ahead of the real one on PATH. Scenarios use it
# when they need deterministic local-remote behavior or injected failures a
# normal local directory would never produce. Its argv log is adapter/debug
# evidence, not part of the high-level differential manifest.
use_fake_rclone() {
  export PATH="$HARNESS_DIR/fake-rclone:$PATH"
  export RCLONE_ARGV_LOG="$WORK/rclone-argv.log"
  : > "$RCLONE_ARGV_LOG"
}

# Appends one fault rule, e.g.
#   inject_fault '{"subcommand": "lsjson", "exit": 1, "stderr": "403 Forbidden"}'
inject_fault() {
  export RCLONE_FAULTS="$WORK/rclone-faults.jsonl"
  printf '%s\n' "$1" >> "$RCLONE_FAULTS"
}
