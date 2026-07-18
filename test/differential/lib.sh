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
    GIT_SFS_CACHE="$CACHE" git_sfs init >/dev/null
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
