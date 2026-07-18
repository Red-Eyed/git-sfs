#!/usr/bin/env bash
# Core local workflow: init, add a tree, commit. Pins symlink targets and the
# content-addressed cache layout with no remote involved.
set -uo pipefail

setup_repo

mkdir -p "$REPO/data/nested"
printf 'train payload\n' > "$REPO/data/train.bin"
printf 'nested payload\n' > "$REPO/data/nested/valid.bin"
# Two paths with identical content, to pin deduplication onto one cache object.
printf 'train payload\n' > "$REPO/data/duplicate.bin"

(
  cd "$REPO"
  record add git_sfs add data
)
commit_all "track dataset"

(
  cd "$REPO"
  record status git_sfs status
  record verify git_sfs verify data
)
