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

func TestMoveFileIntoCacheWithoutCopyingToRepo(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).ImportWithOptions(context.Background(), src, "data/blob.bin", ImportOptions{Move: true}))
	})
	_, err := os.Stat(src)
	require.True(t, os.IsNotExist(err), "source still exists after move")
	dst := filepath.Join(repo, "data", "blob.bin")
	info, err := os.Lstat(dst)
	require.NoError(t, err)
	require.NotZero(t, info.Mode()&os.ModeSymlink, "destination should be a symlink")
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	require.NoError(t, err)
	cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
	require.NoError(t, hash.VerifyFile(context.Background(), cacheFile, h))
	info, err = os.Stat(cacheFile)
	require.NoError(t, err)
	require.Zero(t, info.Mode().Perm()&0o222, "cache file should be read-only")
}

func TestImportResolvesSourceFileSymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	require.NoError(t, os.Symlink(src, link))
	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/blob.bin", ImportOptions{FollowSymlinks: true, Move: true}))
	})
	_, err := os.Lstat(link)
	require.True(t, os.IsNotExist(err), "source symlink should be removed after import")
	_, err = os.Stat(src)
	require.True(t, os.IsNotExist(err), "resolved source should be moved into cache")
	h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob.bin"))
	require.NoError(t, err)
	require.NoError(t, hash.VerifyFile(context.Background(), filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h))
}

func TestImportRejectsSourceSymlinkWithoutFollowFlag(t *testing.T) {
	repo := newRepo(t)
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, src, []byte("large payload"))
	require.NoError(t, os.Symlink(src, link))
	inDir(t, repo, func() {
		require.Error(t, app(&bytes.Buffer{}).Import(context.Background(), link, "data/blob.bin"))
	})
	_, err := os.Lstat(link)
	require.NoError(t, err, "source symlink should remain after failed import")
	_, err = os.Stat(src)
	require.NoError(t, err, "resolved source should remain after failed import")
}

func TestMoveDirectoryIntoCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	linkedSrc := filepath.Join(t.TempDir(), "linked.bin")
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	mustWrite(t, linkedSrc, []byte("linked"))
	require.NoError(t, os.Symlink(linkedSrc, filepath.Join(srcDir, "nested", "linked.bin")))
	require.NoError(t, os.Symlink(filepath.Join(srcDir, "one.bin"), filepath.Join(srcDir, "nested", "one-link.bin")))
	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).ImportWithOptions(context.Background(), srcDir, "data/imported", ImportOptions{FollowSymlinks: true, Move: true}))
	})
	_, err := os.Stat(srcDir)
	require.True(t, os.IsNotExist(err), "source directory should be removed when empty")
	_, err = os.Stat(linkedSrc)
	require.True(t, os.IsNotExist(err), "nested symlink target should be moved into cache")
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin", "data/imported/nested/linked.bin", "data/imported/nested/one-link.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		require.NoError(t, err, rel)
		require.NotZero(t, info.Mode()&os.ModeSymlink, "%s should be a symlink", rel)
	}
}

func TestImportResolvesSourceDirectorySymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	link := filepath.Join(t.TempDir(), "incoming-link")
	writeLocalRemote(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	require.NoError(t, os.Symlink(srcDir, link))
	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/imported", ImportOptions{FollowSymlinks: true, Move: true}))
	})
	_, err := os.Lstat(link)
	require.True(t, os.IsNotExist(err), "source symlink should be removed after import")
	_, err = os.Stat(srcDir)
	require.True(t, os.IsNotExist(err), "resolved source directory should be removed when empty")
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		require.NoError(t, err, rel)
		require.NotZero(t, info.Mode()&os.ModeSymlink, "%s should be a symlink", rel)
	}
}
