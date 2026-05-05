package core

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestPushPullRoundTrip(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, app(&bytes.Buffer{}).Push(context.Background(), ""))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Remove(cacheFile))
		require.NoError(t, app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"))
		require.NoError(t, hash.VerifyFile(cacheFile, h))
	})
}

func TestPushPullRoundTripWithLocalRcloneRemote(t *testing.T) {
	if _, err := exec.LookPath("rclone"); err != nil {
		t.Skip("rclone is not installed")
	}
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	mustWrite(t, filepath.Join(repo, ".git-sfs", "config.toml"), []byte("version = 1\n\n[remotes.default]\nbackend = local\npath = "+remoteDir+"\nconfig = "+filepath.Join(repo, ".git-sfs", "rclone.conf")+"\n\n[settings]\nalgorithm = sha256\n"))
	mustWrite(t, filepath.Join(repo, ".git-sfs", "rclone.conf"), []byte("[local]\ntype = local\n"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, app(&bytes.Buffer{}).Push(context.Background(), ""))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Remove(cacheFile))
		require.NoError(t, app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"))
		require.NoError(t, hash.VerifyFile(cacheFile, h))
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, hash.VerifyFile(remoteFile, h))
	})
}

func TestPullCanRestoreOnlySelectedFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		require.NoError(t, a.Push(context.Background(), ""))
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		require.NoError(t, err)
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "two.bin"))
		require.NoError(t, err)
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		require.NoError(t, os.Remove(cacheOne))
		require.NoError(t, os.Remove(cacheTwo))
		require.NoError(t, a.Pull(context.Background(), "", "data/one.bin"))
		require.NoError(t, hash.VerifyFile(cacheOne, h1))
		_, err = os.Stat(cacheTwo)
		require.True(t, os.IsNotExist(err), "unselected cache file was restored")
	})
}

func TestPullWithMixedPresentAndMissingCacheFiles(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		require.NoError(t, a.Push(context.Background(), ""))
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		require.NoError(t, err)
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "nested", "two.bin"))
		require.NoError(t, err)
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		require.NoError(t, os.Remove(cacheTwo))
		require.NoError(t, a.Pull(context.Background(), "", "data/"))
		require.NoError(t, hash.VerifyFile(cacheOne, h1))
		require.NoError(t, hash.VerifyFile(cacheTwo, h2))
		require.NoError(t, a.Pull(context.Background(), "", "data/one.bin"))
		require.NoError(t, hash.VerifyFile(cacheOne, h1))
	})
}

func TestPullFailsForMissingRemoteFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))
		require.Error(t, a.Pull(context.Background(), "", "data/blob"))
	})
}

func TestSelectedRemoteErrors(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.Error(t, a.Push(context.Background(), "missing"))
	})
}

func TestPullFailsWhenDiskSpaceInsufficient(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	// Use a fake rclone that reports an impossibly large file size so the guard fires.
	bin := t.TempDir()
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then shift 2; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copyto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst" ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -e "$src" ]; then
      printf '[{"Path":"%s","Size":999999999999999}]\n' "$(basename "$src")"
    else
      printf '[]\n'
    fi ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst" ;;
  lsd)
    src="$(map_path "$2")"
    if [ -d "$src" ]; then exit 0; else printf 'directory not found: %s\n' "$src" >&2; exit 1; fi ;;
  *) exit 2 ;;
esac
`)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	writeDataset(t, repo, remoteDir)
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Push(context.Background(), ""))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))
		t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
		err = app(&bytes.Buffer{}).Pull(context.Background(), "", ".")
		require.ErrorContains(t, err, "disk space")
	})
}

func TestPushFailsWhenRcloneNotOnPath(t *testing.T) {
	repo := newRepo(t)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	t.Setenv("PATH", t.TempDir())
	inDir(t, repo, func() {
		err := app(&bytes.Buffer{}).Push(context.Background(), "")
		require.Error(t, err)
		require.True(t, strings.Contains(err.Error(), "not found") || strings.Contains(err.Error(), "no such file"), "unexpected error: %v", err)
	})
}

func TestPushFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		require.ErrorContains(t, a.Push(context.Background(), ""), "does not exist")
	})
}

func TestPushSkipsExistingRemoteFileAndRejectsMissingCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Push(context.Background(), ""))
		require.NoError(t, a.Push(context.Background(), "")) // idempotent
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))
		require.Error(t, a.Push(context.Background(), ""))
	})
}

func TestPullSkipsExistingValidCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Pull(context.Background(), "", "data/blob"))
	})
}

func TestPullFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		require.ErrorContains(t, a.Pull(context.Background(), "", "."), "does not exist")
	})
}

func TestPullRejectsHashMismatch(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	bin := filepath.Join(t.TempDir(), "bin")
	require.NoError(t, os.Mkdir(bin, 0o755))
	// Fake rclone: copy writes wrong content for every file in the list.
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then shift 2; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copy)
    files_from=""; shift
    while [ "$#" -gt 2 ]; do
      case "$1" in
        --ignore-existing) shift ;;
        --files-from) files_from="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    dst_base="$2"
    while IFS= read -r rel; do
      [ -z "$rel" ] && continue
      dst_file="$(map_path "${dst_base}/${rel}")"
      mkdir -p "$(dirname "$dst_file")"
      printf 'wrong content\n' > "$dst_file"
    done < "$files_from" ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -f "$src" ]; then
      size=$(wc -c < "$src" | tr -d ' \t')
      printf '[{"Path":"%s","Size":%s}]\n' "$(basename "$src")" "$size"
    elif [ -e "$src" ]; then
      printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
    else
      printf 'directory not found: %s\n' "$src" >&2; exit 1
    fi ;;
  lsd)
    src="$(map_path "$2")"
    if [ -d "$src" ]; then exit 0; else printf 'directory not found: %s\n' "$src" >&2; exit 1; fi ;;
  *) exit 2 ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Remove(cacheFile))
		require.Error(t, a.Pull(context.Background(), "", "data/blob"))
	})
}

func TestPushFailsForMissingCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))
		require.ErrorIs(t, a.Push(context.Background(), ""), errs.ErrMissingCachedFile)
	})
}

