package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

func TestVerifyUsesParallelRemoteChecks(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteRoot := filepath.Join(t.TempDir(), "remote")
	logPath := filepath.Join(t.TempDir(), "rclone.log")
	bin := filepath.Join(t.TempDir(), "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTimedRcloneTool(t, filepath.Join(bin, "rclone"))
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteRoot)
	t.Setenv("RCLONE_TEST_LOG", logPath)
	writeRcloneDataset(t, repo, "testremote", "dataset")
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		for _, rel := range []string{"data/one.bin", "data/two.bin"} {
			h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, rel))
			if err != nil {
				t.Fatal(err)
			}
			src := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
			dst := filepath.Join(remoteRoot, "dataset", "files", hash.Algorithm, h.Prefix(), h.String())
			mustCopy(t, src, dst)
		}
		start := time.Now()
		if err := a.Verify(context.Background(), "", true, false, "data"); err != nil {
			t.Fatal(err)
		}
		if time.Since(start) > 3200*time.Millisecond {
			t.Fatalf("verify took too long to be parallel: %s", time.Since(start))
		}
	})

	assertParallelStarts(t, logPath, "remote checks")
}

func TestVerifyReportsUnconvertedAndCorruptCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		if err := app(stdout).Verify(context.Background(), "", true, false, "."); err == nil {
			t.Fatal("verify should fail for unconverted file")
		}
		if !strings.Contains(stdout.String(), "unconverted files: 1") ||
			!strings.Contains(stdout.String(), "unconverted file: data/blob") {
			t.Fatalf("verify did not report unconverted file: %q", stdout.String())
		}
		stdout.Reset()
		if err := app(stdout).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Chmod(cacheFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(cacheFile, []byte("corrupt"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(cacheFile, 0o444); err != nil {
			t.Fatal(err)
		}
		stdout.Reset()
		if err := app(stdout).Verify(context.Background(), "", false, true, "."); err == nil {
			t.Fatal("verify should fail for corrupt cache file")
		}
		if !strings.Contains(stdout.String(), "corrupt cache files: 1") ||
			!strings.Contains(stdout.String(), "corrupt cache file: data/blob") {
			t.Fatalf("verify did not report corrupt cache: %q", stdout.String())
		}
	})
}

func TestVerifyDetectsWrongCachePermissions(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		// Make writable without changing content, so hash still matches.
		if err := os.Chmod(cacheFile, 0o644); err != nil {
			t.Fatal(err)
		}
		stdout.Reset()
		if err := app(stdout).Verify(context.Background(), "", false, false, "."); err == nil {
			t.Fatal("verify should fail for writable cache file")
		}
		if !strings.Contains(stdout.String(), "wrong cache permissions: 1") {
			t.Fatalf("verify did not report wrong permissions: %q", stdout.String())
		}
	})
}

func TestVerifyOrphanHint(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		// Write a fake cache file that no symlink references.
		orphanDir := filepath.Join(cacheDir, "files", hash.Algorithm, "ab")
		if err := os.MkdirAll(orphanDir, 0o755); err != nil {
			t.Fatal(err)
		}
		orphanFile := filepath.Join(orphanDir, "ab"+"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
		if err := os.WriteFile(orphanFile, []byte("orphan"), 0o444); err != nil {
			t.Fatal(err)
		}
		stdout.Reset()
		if err := app(stdout).Verify(context.Background(), "", false, false, "."); err != nil {
			t.Fatalf("verify should pass (no issues): %v", err)
		}
		if !strings.Contains(stdout.String(), "orphaned cache object") {
			t.Fatalf("verify did not hint at orphans: %q", stdout.String())
		}
	})
}

func TestVerifyReportsInvalidConfig(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte("version = 1\n\n[settings]\nalgorithm = sha256\n"))
		stdout := &bytes.Buffer{}
		if err := app(stdout).Verify(context.Background(), "", true, false, "."); err == nil {
			t.Fatal("expected invalid config verify")
		}
		if !strings.Contains(stdout.String(), "missing default remote") {
			t.Fatalf("missing verify output: %q", stdout.String())
		}
	})
}

func TestVerifyReportsRemoteProblems(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		stdout := &bytes.Buffer{}
		if err := app(stdout).Verify(context.Background(), "", true, false, "."); err == nil {
			t.Fatal("expected remote verify to fail when remote file is missing")
		}
		if !strings.Contains(stdout.String(), "missing remote files: 1") {
			t.Fatalf("missing remote verify output: %q", stdout.String())
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(remoteFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(remoteFile, []byte("corrupt"), 0o644); err != nil {
			t.Fatal(err)
		}
		stdout.Reset()
		if err := app(stdout).Verify(context.Background(), "", true, true, "."); err == nil {
			t.Fatal("expected remote verify to fail")
		}
		if !strings.Contains(stdout.String(), "corrupt remote files: 1") {
			t.Fatalf("missing corrupt remote output: %q", stdout.String())
		}
	})
}

func TestVerifyPathScopesChecksToSelectedSubtree(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}

		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "nested", "two.bin"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())); err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(remoteDir, "files", hash.Algorithm, h2.Prefix(), h2.String())); err != nil {
			t.Fatal(err)
		}

		if err := a.Verify(context.Background(), "", true, false, "data/one.bin"); err != nil {
			t.Fatalf("verify should ignore unrelated subtree problems: %v", err)
		}
		stdout := &bytes.Buffer{}
		if err := app(stdout).Verify(context.Background(), "", true, false, "data/nested"); err == nil {
			t.Fatal("verify should fail for selected subtree with missing cache file")
		}
		if !strings.Contains(stdout.String(), "missing cache files: 1") ||
			!strings.Contains(stdout.String(), "missing cache file: data/nested/two.bin") {
			t.Fatalf("verify did not scope to nested subtree: %q", stdout.String())
		}
		if strings.Contains(stdout.String(), "data/one.bin") {
			t.Fatalf("verify reported unselected path: %q", stdout.String())
		}
	})
}

func TestVerifyWithoutIntegritySkipsCorruptionChecks(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Chmod(cacheFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(cacheFile, []byte("corrupt-cache"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(cacheFile, 0o444); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(remoteFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(remoteFile, []byte("corrupt-remote"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(remoteFile, 0o444); err != nil {
			t.Fatal(err)
		}
		if err := a.Verify(context.Background(), "", true, false, "."); err != nil {
			t.Fatalf("presence-only verify should ignore corruption: %v", err)
		}
		stdout := &bytes.Buffer{}
		if err := app(stdout).Verify(context.Background(), "", true, true, "."); err == nil {
			t.Fatal("integrity verify should fail for corrupt files")
		}
		if !strings.Contains(stdout.String(), "corrupt cache files: 1") {
			t.Fatalf("integrity verify did not report corrupt cache: %q", stdout.String())
		}
	})
}
