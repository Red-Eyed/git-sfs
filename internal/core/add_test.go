package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestAddAndVerify(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	stdout := &bytes.Buffer{}
	app := app(stdout)
	inDir(t, repo, func() {
		require.NoError(t, app.Add(context.Background(), []string{"data"}))
		require.NoError(t, app.Verify(context.Background(), "", false, false, "."))
	})

	for _, rel := range []string{"data/one.bin", "data/nested/two.bin"} {
		path := filepath.Join(repo, rel)
		info, err := os.Lstat(path)
		require.NoError(t, err)
		require.NotZero(t, info.Mode()&os.ModeSymlink, "%s is not a symlink", rel)
		h, _, err := sfspath.ParseGitSymlink(repo, path)
		require.NoError(t, err)
		require.NoError(t, hash.VerifyFile(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h))
	}
}

func TestVerboseAddOutputsDebug(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))

	stderr := &bytes.Buffer{}
	a := app(&bytes.Buffer{})
	a.Stderr = stderr
	a.Verbose = true
	inDir(t, repo, func() {
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
	})
	got := stderr.String()
	require.True(t, strings.Contains(got, "debug: add: start") && strings.Contains(got, "debug: add: done"),
		"missing verbose add output: %q", got)
}

func TestAddWithCancelledContextLeavesFilesIntact(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "a.bin"), []byte("aaa"))
	mustWrite(t, filepath.Join(repo, "data", "b.bin"), []byte("bbb"))

	inDir(t, repo, func() {
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		require.Error(t, app(&bytes.Buffer{}).Add(ctx, []string{"data"}))
		// Both original files must still exist as regular files.
		for _, name := range []string{"data/a.bin", "data/b.bin"} {
			info, err := os.Lstat(filepath.Join(repo, name))
			require.NoError(t, err, name)
			require.Zero(t, info.Mode()&os.ModeSymlink, "%s was converted despite cancelled context", name)
		}
	})
}

func TestAddWithCacheDirReadOnly(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	filesDir := filepath.Join(cacheDir, "files")
	require.NoError(t, os.MkdirAll(filesDir, 0o755))
	require.NoError(t, os.Chmod(filesDir, 0o555))
	t.Cleanup(func() { os.Chmod(filesDir, 0o755) })
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		require.Error(t, app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}))
		_, err := os.Lstat(filepath.Join(repo, "data", "blob"))
		require.NoError(t, err, "source file must still exist after failed Add")
	})
}
