package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/require"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestMvRewritesRelativeTarget(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob.bin"), []byte("content"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob.bin"}))
		require.NoError(t, a.Mv("data/blob.bin", "nested/sub/blob.bin"))
	})
	_, err := os.Lstat(filepath.Join(repo, "data", "blob.bin"))
	require.True(t, os.IsNotExist(err), "source symlink should be gone")
	dst := filepath.Join(repo, "nested", "sub", "blob.bin")
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	require.NoError(t, err, "destination is not a valid git-sfs symlink")
	require.NoError(t, hash.VerifyFile(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h))
	// Verify the symlink resolves through the cache indirection.
	got, err := os.ReadFile(dst)
	require.NoError(t, err)
	require.Equal(t, "content", string(got))
}

func TestMvIntoDirectory(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob.bin"), []byte("content"))
	require.NoError(t, os.MkdirAll(filepath.Join(repo, "dest"), 0o755))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob.bin"}))
		require.NoError(t, a.Mv("data/blob.bin", "dest"))
	})
	_, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "dest", "blob.bin"))
	require.NoError(t, err, "dest/blob.bin is not a valid git-sfs symlink")
}

func TestMvDirectory(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "sub", "two.bin"), []byte("two"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		require.NoError(t, a.Mv("data", "archive"))
	})
	_, err := os.Lstat(filepath.Join(repo, "data"))
	require.True(t, os.IsNotExist(err), "source directory should be removed after mv")
	for _, rel := range []string{"archive/one.bin", "archive/sub/two.bin"} {
		dst := filepath.Join(repo, rel)
		_, _, err := sfspath.ParseGitSymlink(repo, dst)
		require.NoError(t, err, "%s is not a valid git-sfs symlink", rel)
		got, err := os.ReadFile(dst)
		require.NoError(t, err)
		require.NotEmpty(t, got, "%s: empty content after mv", rel)
	}
}

func TestMvDirectoryIntoExisting(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	require.NoError(t, os.MkdirAll(filepath.Join(repo, "archive"), 0o755))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		// dst exists as directory → POSIX: place src inside it
		require.NoError(t, a.Mv("data", "archive"))
	})
	_, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "archive", "data", "one.bin"))
	require.NoError(t, err, "archive/data/one.bin is not a valid git-sfs symlink")
}

func TestMvWorksOnBrokenSymlinks(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob.bin"), []byte("content"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data/blob.bin"}))
		// Remove all cache files — symlink becomes dangling.
		require.NoError(t, os.RemoveAll(filepath.Join(cacheDir, "files")))
		// mv must still work: it operates on symlink entries, not cache files.
		require.NoError(t, a.Mv("data/blob.bin", "archive/blob.bin"))
	})
	_, err := os.Lstat(filepath.Join(repo, "data", "blob.bin"))
	require.True(t, os.IsNotExist(err), "source symlink should be gone")
	_, _, err = sfspath.ParseGitSymlink(repo, filepath.Join(repo, "archive", "blob.bin"))
	require.NoError(t, err, "destination is not a valid git-sfs symlink")
}

func TestMvDirectoryWorksOnBrokenSymlinks(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "a.bin"), []byte("a"))
	mustWrite(t, filepath.Join(repo, "data", "sub", "b.bin"), []byte("b"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		// Wipe cache to make all symlinks dangling.
		require.NoError(t, os.RemoveAll(filepath.Join(cacheDir, "files")))
		require.NoError(t, a.Mv("data", "archive"))
	})
	_, err := os.Lstat(filepath.Join(repo, "data"))
	require.True(t, os.IsNotExist(err), "source directory should be removed after mv")
	for _, rel := range []string{"archive/a.bin", "archive/sub/b.bin"} {
		_, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, rel))
		require.NoError(t, err, "%s is not a valid git-sfs symlink", rel)
	}
}

func TestMvRejectsNonSymlink(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "plain.txt"), []byte("hello"))
	inDir(t, repo, func() {
		require.Error(t, app(&bytes.Buffer{}).Mv("data/plain.txt", "data/other.txt"))
	})
}
