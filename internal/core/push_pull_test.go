package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

// writeLocalRemote writes a git-sfs config that uses rclone's built-in local
// backend with an absolute path. It also creates a minimal rclone config file
// that registers [local] as type = local, since rclone treats "local:" as a
// named remote (not the built-in) and requires a config entry. Callers create
// remoteDir if the remote must exist before operations run.
func writeLocalRemote(t *testing.T, repo, remoteDir string) {
	t.Helper()
	rcloneCfg := filepath.Join(t.TempDir(), "rclone.conf")
	if err := os.WriteFile(rcloneCfg, []byte("[local]\ntype = local\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	content := "version = 1\n\n[remotes.default]\nbackend = local\npath = " + remoteDir + "\nconfig = " + rcloneCfg + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func TestPushPullRoundTrip(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
		require.NoError(t, hash.VerifyFile(context.Background(), cacheFile, h))
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, hash.VerifyFile(context.Background(), remoteFile, h))
	})
}

func TestPullCanRestoreOnlySelectedFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
		require.NoError(t, hash.VerifyFile(context.Background(), cacheOne, h1))
		_, err = os.Stat(cacheTwo)
		require.True(t, os.IsNotExist(err), "unselected cache file was restored")
	})
}

func TestPullWithMixedPresentAndMissingCacheFiles(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
		require.NoError(t, hash.VerifyFile(context.Background(), cacheOne, h1))
		require.NoError(t, hash.VerifyFile(context.Background(), cacheTwo, h2))
		require.NoError(t, a.Pull(context.Background(), "", "data/one.bin"))
		require.NoError(t, hash.VerifyFile(context.Background(), cacheOne, h1))
	})
}

func TestPullFailsForMissingRemoteFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
	// Use a fake rclone that reports an impossibly large file size so the
	// checkDiskSpace guard fires. The fake only handles lsd/lsjson — it never
	// reaches the copy path because checkDiskSpace returns an error first.
	bin := t.TempDir()
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then shift 2; fi
if [ "${1:-}" = "--progress" ]; then shift; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  lsjson)
    src="$(map_path "$2")"
    if [ -f "$src" ]; then
      printf '[{"Path":"%s","Size":999999999999999}]\n' "$(basename "$src")"
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
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		// Plant the file in the remote directory so lsjson finds it and reports the huge size.
		src := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		dst := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.MkdirAll(filepath.Dir(dst), 0o755))
		mustCopy(t, src, dst)
		require.NoError(t, os.Remove(src))
		t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
		err = app(&bytes.Buffer{}).Pull(context.Background(), "", ".")
		require.ErrorContains(t, err, "disk space")
	})
}

func TestPushFailsWhenRcloneNotOnPath(t *testing.T) {
	repo := newRepo(t)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "nonexistent"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.ErrorContains(t, a.Push(context.Background(), ""), "does not exist")
	})
}

func TestPushSkipsExistingRemoteFileAndRejectsMissingCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "nonexistent"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.ErrorContains(t, a.Pull(context.Background(), "", "."), "does not exist")
	})
}

func TestPullRejectsHashMismatch(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, a.Push(context.Background(), ""))
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Chmod(remoteFile, 0o644))
		require.NoError(t, os.WriteFile(remoteFile, []byte("wrong content"), 0o644))
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Remove(cacheFile))
		require.Error(t, a.Pull(context.Background(), "", "data/blob"))
	})
}

func TestPushFailsForMissingCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
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

func TestPullCleansTmpDirOnStart(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Push(context.Background(), ""))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())

		// Plant a leftover file in cache/tmp, simulating an interrupted previous pull.
		leftover := filepath.Join(cacheDir, "tmp", "rclone-leftover-12345.tmp")
		mustWrite(t, leftover, []byte("partial"))

		// Remove the cache file to force a real download on the next pull.
		require.NoError(t, os.Remove(cacheFile))

		require.NoError(t, a.Pull(context.Background(), "", "data/blob"))

		_, statErr := os.Stat(leftover)
		require.True(t, os.IsNotExist(statErr), "pull must clean cache/tmp leftovers on start")
		require.NoError(t, hash.VerifyFile(context.Background(), cacheFile, h))
	})
}
