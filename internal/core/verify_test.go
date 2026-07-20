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

func TestVerifyReportsUnconvertedAndCorruptCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		require.Error(t, app(stdout).Verify(context.Background(), "", true, false, "."))
		require.Contains(t, stdout.String(), "unconverted files: 1")
		require.Contains(t, stdout.String(), "unconverted file: data/blob")

		stdout.Reset()
		require.NoError(t, app(stdout).Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		require.NoError(t, os.Chmod(cacheFile, 0o644))
		require.NoError(t, os.WriteFile(cacheFile, []byte("corrupt"), 0o644))
		require.NoError(t, os.Chmod(cacheFile, 0o444))

		stdout.Reset()
		require.Error(t, app(stdout).Verify(context.Background(), "", false, true, "."))
		require.Contains(t, stdout.String(), "corrupt cache files: 1")
		require.Contains(t, stdout.String(), "corrupt cache file: data/blob")
	})
}

func TestVerifyDetectsWrongCachePermissions(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		// Make writable without changing content, so hash still matches.
		require.NoError(t, os.Chmod(cacheFile, 0o644))
		stdout.Reset()
		require.Error(t, app(stdout).Verify(context.Background(), "", false, false, "."))
		require.Contains(t, stdout.String(), "wrong cache permissions: 1")
	})
}

func TestVerifyOrphanHint(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		// Write a fake cache file that no symlink references.
		orphanDir := filepath.Join(cacheDir, "files", hash.Algorithm, "ab")
		require.NoError(t, os.MkdirAll(orphanDir, 0o755))
		orphanFile := filepath.Join(orphanDir, "ab"+"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
		require.NoError(t, os.WriteFile(orphanFile, []byte("orphan"), 0o444))
		stdout.Reset()
		require.NoError(t, app(stdout).Verify(context.Background(), "", false, false, "."))
		require.Contains(t, stdout.String(), "orphaned cache object")
	})
}

func TestVerifyReportsInvalidConfig(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	require.NoError(t, os.MkdirAll(remoteDir, 0o755))
	writeLocalRemote(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		require.NoError(t, app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}))
		mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte("version = 1\n\n[settings]\nalgorithm = sha256\n"))
		stdout := &bytes.Buffer{}
		require.Error(t, app(stdout).Verify(context.Background(), "", true, false, "."))
		require.Contains(t, stdout.String(), "missing default remote")
	})
}

func TestVerifyReportsRemoteProblems(t *testing.T) {
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
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())

		stdout := &bytes.Buffer{}
		require.Error(t, app(stdout).Verify(context.Background(), "", true, false, "."))
		require.Contains(t, stdout.String(), "missing remote files: 1")

		require.NoError(t, a.Push(context.Background(), "", "."))
		require.NoError(t, os.Chmod(remoteFile, 0o644))
		require.NoError(t, os.WriteFile(remoteFile, []byte("corrupt"), 0o644))
		stdout.Reset()
		require.Error(t, app(stdout).Verify(context.Background(), "", true, true, "."))
		require.Contains(t, stdout.String(), "corrupt remote files: 1")
	})
}

func TestVerifyPathScopesChecksToSelectedSubtree(t *testing.T) {
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

		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "nested", "two.bin"))
		require.NoError(t, err)
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())))
		require.NoError(t, os.Remove(filepath.Join(remoteDir, "files", hash.Algorithm, h2.Prefix(), h2.String())))

		require.NoError(t, a.Verify(context.Background(), "", true, false, "data/one.bin"))

		stdout := &bytes.Buffer{}
		require.Error(t, app(stdout).Verify(context.Background(), "", true, false, "data/nested"))
		require.Contains(t, stdout.String(), "missing cache files: 1")
		require.Contains(t, stdout.String(), "missing cache file: data/nested/two.bin")
		require.NotContains(t, stdout.String(), "data/one.bin")
	})
}

func TestVerifyWithoutIntegritySkipsCorruptionChecks(t *testing.T) {
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
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())

		require.NoError(t, os.Chmod(cacheFile, 0o644))
		require.NoError(t, os.WriteFile(cacheFile, []byte("corrupt-cache"), 0o644))
		require.NoError(t, os.Chmod(cacheFile, 0o444))
		require.NoError(t, os.Chmod(remoteFile, 0o644))
		require.NoError(t, os.WriteFile(remoteFile, []byte("corrupt-remote"), 0o644))
		require.NoError(t, os.Chmod(remoteFile, 0o444))

		require.NoError(t, a.Verify(context.Background(), "", true, false, "."), "presence-only verify should ignore corruption")

		stdout := &bytes.Buffer{}
		require.Error(t, app(stdout).Verify(context.Background(), "", true, true, "."))
		require.Contains(t, stdout.String(), "corrupt cache files: 1")
	})
}
