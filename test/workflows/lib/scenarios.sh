#!/usr/bin/env bash

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
  local hash_train hash_valid hash_metrics status_out dry_run_out

  mkdir -p "$remote"
  init_repo "$repo_a" "$cache_a"
  write_filesystem_config "$repo_a" "$remote"
  (
    cd "$repo_a"
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

  assert_symlink "$repo_a/data/train-000.tar.zst"
  hash_train="$(hash_from_link "$repo_a" "data/train-000.tar.zst")"
  hash_valid="$(hash_from_link "$repo_a" "data/validation/one.bin")"
  hash_metrics="$(hash_from_link "$repo_a" "data/validation/two.bin")"
  [ -f "$(cache_file_for "$cache_a" "$hash_train")" ] || fail "missing cached train shard"
  [ -f "$remote/files/sha256/${hash_train:0:2}/$hash_train" ] || fail "missing remote train shard"

  git clone -q "$repo_a" "$repo_b"
  git_setup_user "$repo_b"
  (
    cd "$repo_b"
    git_sfs --cache "$cache_b" setup >/dev/null
    git_sfs pull >/dev/null
    git_sfs verify >/dev/null
  )
  assert_eq "$(cat "$repo_b/data/train-000.tar.zst")" "train shard" "full clone pull"
  assert_eq "$(cat "$repo_b/data/validation/one.bin")" "validation one" "full directory pull"

  rm -f "$(cache_file_for "$cache_b" "$hash_train")"
  (
    cd "$repo_b"
    git_sfs pull data/train-000.tar.zst >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$cache_b" "$hash_train")" ] || fail "selected file was not restored"

  rm -f "$(cache_file_for "$cache_b" "$hash_valid")" "$(cache_file_for "$cache_b" "$hash_metrics")"
  (
    cd "$repo_b"
    git_sfs pull data/validation/ >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$cache_b" "$hash_valid")" ] || fail "selected directory file was not restored"
  [ -f "$(cache_file_for "$cache_b" "$hash_metrics")" ] || fail "selected directory file was not restored"

  git clone -q "$repo_a" "$repo_c"
  (
    cd "$repo_c"
    GIT_SFS_CACHE="$temp_cache" git_sfs setup >/dev/null
    GIT_SFS_CACHE="$temp_cache" git_sfs pull data/train-000.tar.zst >/dev/null
  )
  [ -f "$(cache_file_for "$temp_cache" "$hash_train")" ] || fail "temporary cache pull failed"

  git clone -q "$repo_a" "$repo_d"
  (
    cd "$repo_d"
    git_sfs --cache "$shared_cache" setup >/dev/null
    git_sfs pull data/train-000.tar.zst >/dev/null
  )
  [ -f "$(cache_file_for "$shared_cache" "$hash_train")" ] || fail "shared cache pull failed"

  (
    cd "$repo_b"
    rm -f .git-sfs/cache
    git_sfs --cache "$shared_cache" setup >/dev/null
    git_sfs pull >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$shared_cache" "$hash_train")" ] || fail "cache move did not populate new cache"
  [ -f "$(cache_file_for "$shared_cache" "$hash_valid")" ] || fail "cache move did not populate new cache"
  [ -f "$(cache_file_for "$shared_cache" "$hash_metrics")" ] || fail "cache move did not populate new cache"

  (
    cd "$repo_b"
    rm -f .git-sfs/cache
    git_sfs --cache "$shared_cache" setup >/dev/null
    git_sfs verify >/dev/null
  )

  rm -f "$(cache_file_for "$shared_cache" "$hash_train")"
  (
    cd "$repo_b"
    git_sfs pull data/train-000.tar.zst >/dev/null
    git_sfs verify >/dev/null
  )
  [ -f "$(cache_file_for "$shared_cache" "$hash_train")" ] || fail "cache loss recovery failed"

  mkdir -p "$shared_cache/files/sha256/ff"
  printf "orphan\n" > "$shared_cache/files/sha256/ff/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
  (
    cd "$repo_b"
    dry_run_out="$(git_sfs gc --dry-run)"
    assert_contains "$dry_run_out" "gc dry-run would remove 1 file(s)" "gc dry-run summary"
    git_sfs gc --files >/dev/null
  )
  [ ! -e "$shared_cache/files/sha256/ff/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" ] || fail "gc did not delete orphan"
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
