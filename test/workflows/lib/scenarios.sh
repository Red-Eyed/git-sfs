#!/usr/bin/env bash

seed_filesystem_repo() {
  local repo="$1"
  local remote="$2"
  local status_out

  mkdir -p "$remote"
  init_repo "$repo" "$3"
  write_filesystem_config "$repo" "$remote"
  (
    cd "$repo"
    # Start from a normal repo state so later clones exercise the same tracked
    # symlinks users would commit and push through Git.
    git add .git-sfs/config.toml .gitignore
    git commit -qm "initialize git-sfs"
    mkdir -p data
    printf "train shard\n" > data/train-000.tar.zst
    git_sfs add data/train-000.tar.zst >/dev/null
    status_out="$(git_sfs status)"
    assert_contains "$status_out" "tracked symlinks: 1" "single-file status"
    git add data/train-000.tar.zst
    git commit -qm "track train shard"
    git_sfs push >/dev/null

    mkdir -p data/validation
    printf "validation one\n" > data/validation/one.bin
    printf "validation two\n" > data/validation/two.bin
    git_sfs add data/validation >/dev/null
    git add data/validation
    git commit -qm "track validation shards"
    git_sfs push >/dev/null

    status_out="$(git_sfs status)"
    assert_contains "$status_out" "tracked symlinks: 3" "directory status"
    git diff --cached --stat >/dev/null
  )
}

assert_filesystem_seeded() {
  local repo="$1"
  local cache="$2"
  local remote="$3"
  local hash_train="$4"

  assert_symlink "$repo/data/train-000.tar.zst"
  [ -f "$(cache_file_for "$cache" "$hash_train")" ] || fail "missing cached train shard"
  [ -f "$remote/files/sha256/${hash_train:0:2}/$hash_train" ] || fail "missing remote train shard"
}

clone_and_pull_repo() {
  local src_repo="$1"
  local dst_repo="$2"
  local cache="$3"

  git clone -q "$src_repo" "$dst_repo"
  git_setup_user "$dst_repo"
  (
    cd "$dst_repo"
    git_sfs --cache "$cache" setup >/dev/null
    git_sfs pull >/dev/null
    git_sfs verify >/dev/null
  )
}

assert_clone_contents() {
  local repo="$1"
  assert_eq "$(cat "$repo/data/train-000.tar.zst")" "train shard" "full clone pull"
  assert_eq "$(cat "$repo/data/validation/one.bin")" "validation one" "full directory pull"
}

restore_selected_file() {
  local repo="$1"
  local cache="$2"
  local hash="$3"
  local path="$4"

  rm -f "$(cache_file_for "$cache" "$hash")"
  (
    cd "$repo"
    git_sfs pull "$path" >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$cache" "$hash")" ] || fail "selected file was not restored"
}

restore_selected_directory() {
  local repo="$1"
  local cache="$2"
  local hash_a="$3"
  local hash_b="$4"
  local path="$5"

  rm -f "$(cache_file_for "$cache" "$hash_a")" "$(cache_file_for "$cache" "$hash_b")"
  (
    cd "$repo"
    git_sfs pull "$path" >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$cache" "$hash_a")" ] || fail "selected directory file was not restored"
  [ -f "$(cache_file_for "$cache" "$hash_b")" ] || fail "selected directory file was not restored"
}

pull_with_env_cache() {
  local src_repo="$1"
  local dst_repo="$2"
  local cache="$3"
  local path="$4"

  git clone -q "$src_repo" "$dst_repo"
  (
    cd "$dst_repo"
    GIT_SFS_CACHE="$cache" git_sfs setup >/dev/null
    GIT_SFS_CACHE="$cache" git_sfs pull "$path" >/dev/null
  )
}

pull_with_flag_cache() {
  local src_repo="$1"
  local dst_repo="$2"
  local cache="$3"
  local path="$4"

  git clone -q "$src_repo" "$dst_repo"
  (
    cd "$dst_repo"
    git_sfs --cache "$cache" setup >/dev/null
    git_sfs pull "$path" >/dev/null
  )
}

rebind_repo_cache() {
  local repo="$1"
  local cache="$2"
  local pull_path="${3:-__skip_pull__}"

  (
    cd "$repo"
    rm -f .git-sfs/cache
    git_sfs --cache "$cache" setup >/dev/null
    if [ "$pull_path" != "__skip_pull__" ]; then
      git_sfs pull "$pull_path" >/dev/null
    fi
    git_sfs verify >/dev/null
  )
}

assert_cache_populated() {
  local cache="$1"
  shift
  local hash
  for hash in "$@"; do
    [ -f "$(cache_file_for "$cache" "$hash")" ] || fail "cache move did not populate new cache"
  done
}

assert_gc_removes_orphan() {
  local repo="$1"
  local cache="$2"
  local dry_run_out
  local orphan="$cache/files/sha256/ff/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"

  mkdir -p "$cache/files/sha256/ff"
  printf "orphan\n" > "$orphan"
  (
    cd "$repo"
    dry_run_out="$(git_sfs gc --dry-run)"
    assert_contains "$dry_run_out" "gc dry-run would remove 1 file(s)" "gc dry-run summary"
    git_sfs gc --files >/dev/null
  )
  [ ! -e "$orphan" ] || fail "gc did not delete orphan"
}

scenario_filesystem_workflows() {
  note "exercise documented filesystem workflows"
  local repo_a="$WORK/repo-a"
  local repo_b="$WORK/repo-b"
  local repo_c="$WORK/repo-c"
  local repo_d="$WORK/repo-d"
  local cache_a="$WORK/cache-a"
  local cache_b="$WORK/cache-b"
  local shared_cache="$WORK/shared-cache"
  local temp_cache="$WORK/temp-cache"
  local remote="$REMOTE_ROOT/filesystem"
  local hash_train hash_valid hash_metrics

  seed_filesystem_repo "$repo_a" "$remote" "$cache_a"

  hash_train="$(hash_from_link "$repo_a" "data/train-000.tar.zst")"
  hash_valid="$(hash_from_link "$repo_a" "data/validation/one.bin")"
  hash_metrics="$(hash_from_link "$repo_a" "data/validation/two.bin")"
  assert_filesystem_seeded "$repo_a" "$cache_a" "$remote" "$hash_train"

  # A fresh clone with an empty cache should be able to materialize everything
  # from the committed symlinks plus the configured remote.
  clone_and_pull_repo "$repo_a" "$repo_b" "$cache_b"
  assert_clone_contents "$repo_b"

  # Removing one cached object should only require fetching that object again.
  restore_selected_file "$repo_b" "$cache_b" "$hash_train" "data/train-000.tar.zst"

  # Pulling a directory should repopulate only the files needed under that path.
  restore_selected_directory "$repo_b" "$cache_b" "$hash_valid" "$hash_metrics" "data/validation/"

  # The cache may come from the environment instead of the repo-local symlink.
  pull_with_env_cache "$repo_a" "$repo_c" "$temp_cache" "data/train-000.tar.zst"
  [ -f "$(cache_file_for "$temp_cache" "$hash_train")" ] || fail "temporary cache pull failed"

  # The explicit flag path is the other supported override and should work the
  # same way for a brand-new clone.
  pull_with_flag_cache "$repo_a" "$repo_d" "$shared_cache" "data/train-000.tar.zst"
  [ -f "$(cache_file_for "$shared_cache" "$hash_train")" ] || fail "shared cache pull failed"

  # Re-pointing an existing repo at a new cache should be safe and should not
  # require any hidden state beyond the tracked symlinks and remote bytes.
  rebind_repo_cache "$repo_b" "$shared_cache" "."
  assert_cache_populated "$shared_cache" "$hash_train" "$hash_valid" "$hash_metrics"
  rebind_repo_cache "$repo_b" "$shared_cache"

  # A missing object in the replacement cache should still be recoverable from
  # the remote without disturbing the rest of the cache.
  restore_selected_file "$repo_b" "$shared_cache" "$hash_train" "data/train-000.tar.zst"

  # GC only reasons about Git-visible references, so an unreferenced cache file
  # created by hand should be reported and then deleted.
  assert_gc_removes_orphan "$repo_b" "$shared_cache"
}

scenario_import_workflows() {
  note "exercise import workflows"
  local repo="$WORK/repo-import"
  local cache="$WORK/cache-import"
  local remote="$REMOTE_ROOT/import"
  local outside="$WORK/outside"
  local src_hash

  mkdir -p "$remote" "$outside"
  init_repo "$repo" "$cache"
  write_filesystem_config "$repo" "$remote"
  (
    cd "$repo"
    git add .git-sfs/config.toml .gitignore
    git commit -qm "initialize import repo"
  )

  mkdir -p "$outside/dataset/sub"
  printf "large payload\n" > "$outside/dataset/sub/blob.bin"
  (
    cd "$repo"
    # Import should consume the source tree rather than duplicate it inside the
    # repository when a rename is possible.
    git_sfs import "$outside/dataset" data/dataset >/dev/null
    git add data/dataset
    git commit -qm "track imported dataset"
    git_sfs push >/dev/null
  )
  [ ! -e "$outside/dataset" ] || fail "import should move the source tree"
  assert_eq "$(cat "$repo/data/dataset/sub/blob.bin")" "large payload" "imported dataset content"

  mkdir -p "$outside/follow"
  printf "resolved content\n" > "$outside/source.bin"
  ln -s "$outside/source.bin" "$outside/follow/source-link.bin"
  (
    cd "$repo"
    # With -L, the imported bytes should come from the symlink target while the
    # source-side entry is still consumed as part of the move/import workflow.
    git_sfs import -L "$outside/follow" data/followed >/dev/null
    git add data/followed
    git commit -qm "track followed imports"
    git_sfs push >/dev/null
  )
  [ ! -e "$outside/follow/source-link.bin" ] || fail "followed source symlink should be consumed"
  assert_eq "$(cat "$repo/data/followed/source-link.bin")" "resolved content" "followed import content"

  src_hash="$(hash_from_link "$repo" "data/followed/source-link.bin")"
  [ -f "$remote/files/sha256/${src_hash:0:2}/$src_hash" ] || fail "import push missing remote file"
}

scenario_rclone_workflow() {
  note "exercise rclone workflow"
  local repo="$WORK/repo-rclone"
  local clone="$WORK/repo-rclone-clone"
  local cache="$WORK/cache-rclone"
  local clone_cache="$WORK/cache-rclone-clone"
  local remote_root="$REMOTE_ROOT/rclone"
  local rclone_conf="$WORK/rclone.conf"

  mkdir -p "$remote_root"
  export RCLONE_TEST_ROOT="$remote_root"
  # Keep the workflow black-box from git-sfs's perspective while avoiding any
  # network or cloud account dependency in CI.
  install_fake_rclone
  init_repo "$repo" "$cache"

  cat > "$rclone_conf" <<EOF
[testremote]
type = local
EOF

  write_rclone_config "$repo" "testremote" "dataset" "$rclone_conf"
  (
    cd "$repo"
    git add .git-sfs/config.toml .gitignore
    git commit -qm "initialize rclone repo"
    mkdir -p data/nested
    printf "one\n" > data/one.bin
    printf "two\n" > data/nested/two.bin
    git_sfs add data >/dev/null
    git add data
    git commit -qm "track data for rclone"
    git_sfs push >/dev/null
    git_sfs verify --remote >/dev/null
  )

  # The clone path mirrors the documented user workflow: clone with Git, bind a
  # local cache, then materialize tracked files from the remote.
  git clone -q "$repo" "$clone"
  (
    cd "$clone"
    git_sfs --cache "$clone_cache" setup >/dev/null
    git_sfs pull data/ >/dev/null
    git_sfs verify >/dev/null
    git_sfs status --remote >/dev/null
  )
  assert_eq "$(cat "$clone/data/one.bin")" "one" "rclone roundtrip file one"
  assert_eq "$(cat "$clone/data/nested/two.bin")" "two" "rclone roundtrip file two"
}
