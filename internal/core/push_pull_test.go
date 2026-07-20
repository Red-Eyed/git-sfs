package core

import (
	"bytes"
	"context"
	"fmt"
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
		require.NoError(t, app(&bytes.Buffer{}).Push(context.Background(), "", "."))
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
		require.NoError(t, a.Push(context.Background(), "", "."))
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
		require.NoError(t, a.Push(context.Background(), "", "."))
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
		require.Error(t, a.Push(context.Background(), "missing", "."))
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
while [ "${1:-}" = "--config" ] || [ "${1:-}" = "--progress" ] || [ "${1:-}" = "--stats" ] || [ "${1:-}" = "--stats-one-line" ]; do
  case "$1" in
    --config|--stats) shift 2 ;;
    *) shift ;;
  esac
done
cmd="${1:-}"
map_path() {
  case "$1" in
    localtest:*) printf '%s%s\n' "$RCLONE_TEST_ROOT" "${1#localtest:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  lsjson)
    shift
    recursive=false
    if [ "${1:-}" = "--recursive" ]; then recursive=true; shift; fi
    src="$(map_path "${1:-}")"
    if $recursive; then
      if [ -d "$src" ]; then
        tmp="$(mktemp)"
        find "$src" -type f > "$tmp" 2>/dev/null || true
        out='['
        sep=''
        while IFS= read -r f; do
          name="$(basename "$f")"
          out="${out}${sep}{\"Name\":\"${name}\",\"Size\":999999999999999}"
          sep=','
        done < "$tmp"
        rm -f "$tmp"
        printf '%s]\n' "${out}"
      else
        printf '[]\n'
      fi
    else
      if [ -f "$src" ]; then
        printf '[{"Path":"%s","Size":999999999999999}]\n' "$(basename "$src")"
      elif [ -e "$src" ]; then
        printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
      else
        printf 'directory not found: %s\n' "$src" >&2; exit 1
      fi
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
		err := app(&bytes.Buffer{}).Push(context.Background(), "", ".")
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
		require.ErrorContains(t, a.Push(context.Background(), "", "."), "does not exist")
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
		require.NoError(t, a.Push(context.Background(), "", "."))
		require.NoError(t, a.Push(context.Background(), "", ".")) // idempotent
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))
		require.Error(t, a.Push(context.Background(), "", "."))
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
		require.NoError(t, a.Push(context.Background(), "", "."))
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
		require.ErrorIs(t, a.Push(context.Background(), "", "."), errs.ErrMissingCachedFile)
	})
}

// A partially-pulled dataset must still be pushable: pushing a subtree whose
// files are cached must not be blocked by a sibling subtree the user never
// pulled. Without a path argument, push scans the whole repo and aborts on the
// uncached sibling.
func TestPushCanUploadOnlySelectedPath(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "want", "blob"), []byte("bytes to push"))
	mustWrite(t, filepath.Join(repo, "other", "blob"), []byte("bytes never pulled"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"want/blob", "other/blob"}))

		want, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "want", "blob"))
		require.NoError(t, err)
		other, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "other", "blob"))
		require.NoError(t, err)
		// Drop the sibling's cache file to mimic a subtree that was never pulled.
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, other.Prefix(), other.String())))

		require.NoError(t, a.Push(context.Background(), "", "want"))

		wantRemote := filepath.Join(remoteDir, "files", hash.Algorithm, want.Prefix(), want.String())
		require.NoError(t, hash.VerifyFile(context.Background(), wantRemote, want))
		otherRemote := filepath.Join(remoteDir, "files", hash.Algorithm, other.Prefix(), other.String())
		require.NoFileExists(t, otherRemote)

		// The whole-repo push still fails, and names the offending path.
		err = a.Push(context.Background(), "", ".")
		require.ErrorIs(t, err, errs.ErrMissingCachedFile)
		require.ErrorContains(t, err, filepath.Join("other", "blob"))
	})
}

// --skip-missing trades completeness for progress: it must upload every cached
// file, leave the uncached ones alone, and say plainly on stderr that the remote
// is now an incomplete copy.
func TestPushSkipMissingUploadsWhatIsCached(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "want", "blob"), []byte("bytes to push"))
	mustWrite(t, filepath.Join(repo, "other", "blob"), []byte("bytes never pulled"))

	inDir(t, repo, func() {
		stderr := &bytes.Buffer{}
		a := appWithStderr(&bytes.Buffer{}, stderr)
		require.NoError(t, a.Add(context.Background(), []string{"want/blob", "other/blob"}))

		want, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "want", "blob"))
		require.NoError(t, err)
		other, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "other", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, other.Prefix(), other.String())))

		opts := PushOptions{SkipMissing: true}
		require.NoError(t, a.PushWithOptions(context.Background(), "", ".", opts))

		wantRemote := filepath.Join(remoteDir, "files", hash.Algorithm, want.Prefix(), want.String())
		require.NoError(t, hash.VerifyFile(context.Background(), wantRemote, want))
		otherRemote := filepath.Join(remoteDir, "files", hash.Algorithm, other.Prefix(), other.String())
		require.NoFileExists(t, otherRemote)

		require.Contains(t, stderr.String(), "skipped 1 file(s) referenced by 1 symlink(s)")
		require.Contains(t, stderr.String(), filepath.Join("other", "blob"))
	})
}

// With nothing cached at all, --skip-missing must still succeed and warn rather
// than uploading an empty set silently.
func TestPushSkipMissingWithNothingCached(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	inDir(t, repo, func() {
		stderr := &bytes.Buffer{}
		a := appWithStderr(&bytes.Buffer{}, stderr)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))

		opts := PushOptions{SkipMissing: true}
		require.NoError(t, a.PushWithOptions(context.Background(), "", ".", opts))
		require.Contains(t, stderr.String(), "skipped 1 file(s) referenced by 1 symlink(s)")
	})
}

// Several symlinks can share one cached object. The skip report must count and
// list the affected working-tree paths, not just the unique objects, or it
// understates how much of the tree is missing from the remote.
func TestPushSkipMissingReportsEveryAffectedPath(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	for _, dir := range []string{"a", "b", "c"} {
		mustWrite(t, filepath.Join(repo, dir, "blob"), []byte("shared content"))
	}

	inDir(t, repo, func() {
		stderr := &bytes.Buffer{}
		a := appWithStderr(&bytes.Buffer{}, stderr)
		require.NoError(t, a.Add(context.Background(), []string{"a/blob", "b/blob", "c/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "a", "blob"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))

		require.NoError(t, a.PushWithOptions(context.Background(), "", ".", PushOptions{SkipMissing: true}))

		out := stderr.String()
		require.Contains(t, out, "skipped 1 file(s) referenced by 3 symlink(s)")
		for _, dir := range []string{"a", "b", "c"} {
			require.Contains(t, out, filepath.Join(dir, "blob"))
		}
	})
}

// The per-path listing is capped so a heavily partial checkout cannot bury the
// result in thousands of lines.
func TestPushSkipMissingCapsThePathListing(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	total := maxSkippedListed + 5
	paths := make([]string, 0, total)
	for i := range total {
		p := filepath.Join("data", fmt.Sprintf("blob-%02d", i))
		mustWrite(t, filepath.Join(repo, p), fmt.Appendf(nil, "payload %d", i))
		paths = append(paths, p)
	}

	inDir(t, repo, func() {
		stderr := &bytes.Buffer{}
		a := appWithStderr(&bytes.Buffer{}, stderr)
		require.NoError(t, a.Add(context.Background(), paths))
		require.NoError(t, os.RemoveAll(filepath.Join(cacheDir, "files")))

		require.NoError(t, a.PushWithOptions(context.Background(), "", ".", PushOptions{SkipMissing: true}))

		out := stderr.String()
		require.Contains(t, out, fmt.Sprintf("referenced by %d symlink(s)", total))
		require.Contains(t, out, "... and 5 more")
		require.Equal(t, maxSkippedListed, strings.Count(out, "  data/blob-"))
	})
}

// A corrupt cache file must be treated as missing, never uploaded as-is.
func TestPushSkipMissingSkipsCorruptCacheFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	inDir(t, repo, func() {
		stderr := &bytes.Buffer{}
		a := appWithStderr(&bytes.Buffer{}, stderr)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Chmod(cacheFile, 0o644))
		require.NoError(t, os.WriteFile(cacheFile, []byte("tampered"), 0o644))

		opts := PushOptions{SkipMissing: true}
		require.NoError(t, a.PushWithOptions(context.Background(), "", ".", opts))

		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoFileExists(t, remoteFile)
		require.Contains(t, stderr.String(), "skipped 1 file(s) referenced by 1 symlink(s)")
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
		require.NoError(t, a.Push(context.Background(), "", "."))
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
