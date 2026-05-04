package core

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"git-sfs/internal/errs"
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
		if err := app.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := app.Verify(context.Background(), "", false, false, "."); err != nil {
			t.Fatal(err)
		}
	})

	for _, rel := range []string{"data/one.bin", "data/nested/two.bin"} {
		path := filepath.Join(repo, rel)
		info, err := os.Lstat(path)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s is not a symlink", rel)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, path)
		if err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h); err != nil {
			t.Fatal(err)
		}
	}
}

func TestVerboseAddOutputsDebug(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))

	stderr := &bytes.Buffer{}
	a := app(&bytes.Buffer{})
	a.Stderr = stderr
	a.Verbose = true
	inDir(t, repo, func() {
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
	})
	got := stderr.String()
	if !strings.Contains(got, "debug: add: start") || !strings.Contains(got, "debug: add: done") {
		t.Fatalf("missing verbose add output: %q", got)
	}
}

func TestPushPullRoundTrip(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPushPullRoundTripWithLocalRcloneRemote(t *testing.T) {
	if _, err := exec.LookPath("rclone"); err != nil {
		t.Skip("rclone is not installed")
	}
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	if err := os.MkdirAll(remoteDir, 0o755); err != nil {
		t.Fatal(err)
	}
	mustWrite(t, filepath.Join(repo, ".git-sfs", "config.toml"), []byte("version = 1\n\n[remotes.default]\nbackend = local\npath = "+remoteDir+"\nconfig = "+filepath.Join(repo, ".git-sfs", "rclone.conf")+"\n\n[settings]\nalgorithm = sha256\n"))
	mustWrite(t, filepath.Join(repo, ".git-sfs", "rclone.conf"), []byte("[local]\ntype = local\n"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("large bytes"))

	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := app(&bytes.Buffer{}).Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			t.Fatal(err)
		}
		remoteFile := filepath.Join(remoteDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := hash.VerifyFile(remoteFile, h); err != nil {
			t.Fatal(err)
		}
	})
}

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
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		if err != nil {
			t.Fatal(err)
		}
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "two.bin"))
		if err != nil {
			t.Fatal(err)
		}
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		if err := os.Remove(cacheOne); err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(cacheTwo); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/one.bin"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
		if _, err := os.Stat(cacheTwo); !os.IsNotExist(err) {
			t.Fatalf("unselected cache file was restored: %v", err)
		}
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
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h1, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "one.bin"))
		if err != nil {
			t.Fatal(err)
		}
		h2, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "nested", "two.bin"))
		if err != nil {
			t.Fatal(err)
		}
		cacheOne := filepath.Join(cacheDir, "files", hash.Algorithm, h1.Prefix(), h1.String())
		cacheTwo := filepath.Join(cacheDir, "files", hash.Algorithm, h2.Prefix(), h2.String())
		if err := os.Remove(cacheTwo); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheTwo, h2); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/one.bin"); err != nil {
			t.Fatal(err)
		}
		if err := hash.VerifyFile(cacheOne, h1); err != nil {
			t.Fatal(err)
		}
	})
}

func TestMoveFileIntoCacheWithoutCopyingToRepo(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), src, "data/blob.bin", ImportOptions{Move: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("source still exists after move: %v", err)
	}
	dst := filepath.Join(repo, "data", "blob.bin")
	info, err := os.Lstat(dst)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode()&os.ModeSymlink == 0 {
		t.Fatal("destination should be a symlink")
	}
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	if err != nil {
		t.Fatal(err)
	}
	cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
	if err := hash.VerifyFile(cacheFile, h); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(cacheFile); err != nil || info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("cache file should exist and be read-only: info=%v err=%v", info, err)
	}
}

func TestImportResolvesSourceFileSymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, src, []byte("large payload"))
	if err := os.Symlink(src, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/blob.bin", ImportOptions{FollowSymlinks: true, Move: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(link); !os.IsNotExist(err) {
		t.Fatalf("source symlink should be removed after import: %v", err)
	}
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("resolved source should be moved into cache: %v", err)
	}
	dst := filepath.Join(repo, "data", "blob.bin")
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	if err != nil {
		t.Fatal(err)
	}
	cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
	if err := hash.VerifyFile(cacheFile, h); err != nil {
		t.Fatal(err)
	}
}

func TestImportRejectsSourceSymlinkWithoutFollowFlag(t *testing.T) {
	repo := newRepo(t)
	src := filepath.Join(t.TempDir(), "outside.bin")
	link := filepath.Join(t.TempDir(), "outside-link.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, src, []byte("large payload"))
	if err := os.Symlink(src, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Import(context.Background(), link, "data/blob.bin"); err == nil {
			t.Fatal("expected source symlink import without -L to fail")
		}
	})
	if _, err := os.Lstat(link); err != nil {
		t.Fatalf("source symlink should remain after failed import: %v", err)
	}
	if _, err := os.Stat(src); err != nil {
		t.Fatalf("resolved source should remain after failed import: %v", err)
	}
}

func TestMoveDirectoryIntoCache(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	linkedSrc := filepath.Join(t.TempDir(), "linked.bin")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	mustWrite(t, linkedSrc, []byte("linked"))
	if err := os.Symlink(linkedSrc, filepath.Join(srcDir, "nested", "linked.bin")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(srcDir, "one.bin"), filepath.Join(srcDir, "nested", "one-link.bin")); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), srcDir, "data/imported", ImportOptions{FollowSymlinks: true, Move: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Stat(srcDir); !os.IsNotExist(err) {
		t.Fatalf("source directory should be removed when empty: %v", err)
	}
	if _, err := os.Stat(linkedSrc); !os.IsNotExist(err) {
		t.Fatalf("nested symlink target should be moved into cache: %v", err)
	}
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin", "data/imported/nested/linked.bin", "data/imported/nested/one-link.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s should be a symlink: info=%v err=%v", rel, info, err)
		}
	}
}

func TestImportResolvesSourceDirectorySymlink(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	srcDir := filepath.Join(t.TempDir(), "incoming")
	link := filepath.Join(t.TempDir(), "incoming-link")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(srcDir, "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(srcDir, "nested", "two.bin"), []byte("two"))
	if err := os.Symlink(srcDir, link); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).ImportWithOptions(context.Background(), link, "data/imported", ImportOptions{FollowSymlinks: true, Move: true}); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(link); !os.IsNotExist(err) {
		t.Fatalf("source symlink should be removed after import: %v", err)
	}
	if _, err := os.Stat(srcDir); !os.IsNotExist(err) {
		t.Fatalf("resolved source directory should be removed when empty: %v", err)
	}
	for _, rel := range []string{"data/imported/one.bin", "data/imported/nested/two.bin"} {
		info, err := os.Lstat(filepath.Join(repo, rel))
		if err != nil || info.Mode()&os.ModeSymlink == 0 {
			t.Fatalf("%s should be a symlink: info=%v err=%v", rel, info, err)
		}
	}
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
		if err := app(stdout).Verify(context.Background(), "", false, true, "."); err == nil {
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

func TestAddWithCancelledContextLeavesFilesIntact(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "a.bin"), []byte("aaa"))
	mustWrite(t, filepath.Join(repo, "data", "b.bin"), []byte("bbb"))

	inDir(t, repo, func() {
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		err := app(&bytes.Buffer{}).Add(ctx, []string{"data"})
		if err == nil {
			t.Fatal("expected error with cancelled context")
		}
		// Both original files must still exist as regular files.
		for _, name := range []string{"data/a.bin", "data/b.bin"} {
			info, err := os.Lstat(filepath.Join(repo, name))
			if err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			if info.Mode()&os.ModeSymlink != 0 {
				t.Fatalf("%s was converted despite cancelled context", name)
			}
		}
	})
}

func TestInitSetupAndGitignore(t *testing.T) {
	repo := newRepo(t)
	inDir(t, repo, func() {
		stdout := &bytes.Buffer{}
		a := app(stdout)
		if err := a.Init(context.Background(), false); err != nil {
			t.Fatal(err)
		}
		if err := a.Setup(context.Background()); err != nil {
			t.Fatal(err)
		}
		if target, err := os.Readlink(filepath.Join(repo, ".git-sfs", "cache")); err != nil || target == "" {
			t.Fatalf("cache symlink missing: target=%q err=%v", target, err)
		}
		if info, err := os.Stat(filepath.Join(repo, ".git-sfs", ".cache", "files")); err != nil || !info.IsDir() {
			t.Fatalf("default cache missing: %v", err)
		}
		if err := a.Init(context.Background(), false); err == nil {
			t.Fatal("init should not overwrite config")
		}
		if err := a.Init(context.Background(), true); err != nil {
			t.Fatal(err)
		}
		gitignore, err := os.ReadFile(filepath.Join(repo, ".gitignore"))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(gitignore), ".git-sfs/cache") {
			t.Fatalf(".gitignore missing .git-sfs/: %q", gitignore)
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
		if err := os.Chmod(remoteFile, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(remoteFile, []byte("corrupt-remote"), 0o644); err != nil {
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

func TestPullFailsForMissingRemoteFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
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
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err == nil {
			t.Fatal("expected missing remote file error")
		}
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
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), "missing"); err == nil {
			t.Fatal("expected missing remote error")
		}
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

	// Push using the standard fake (need a separate setup to push first).
	// Easier: manually create the remote file so pull has something to pull.
	writeDataset(t, repo, remoteDir) // sets up config and standard fake rclone temporarily
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
		// Remove cache so pull will try to fetch.
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		// Now override PATH to use the huge-size fake rclone.
		t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
		err = app(&bytes.Buffer{}).Pull(context.Background(), "", ".")
		if err == nil {
			t.Fatal("expected disk space error")
		}
		if !strings.Contains(err.Error(), "disk space") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPushFailsWhenRcloneNotOnPath(t *testing.T) {
	repo := newRepo(t)
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir) // sets config.toml and a fake rclone on PATH
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	// Override PATH to an empty directory so rclone cannot be found.
	emptyBin := t.TempDir()
	t.Setenv("PATH", emptyBin)
	inDir(t, repo, func() {
		err := app(&bytes.Buffer{}).Push(context.Background(), "")
		if err == nil {
			t.Fatal("expected error when rclone is not on PATH")
		}
		if !strings.Contains(err.Error(), "not found") && !strings.Contains(err.Error(), "no such file") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPushFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	// Set up with a valid remote so that Add works, then switch RCLONE_TEST_ROOT
	// to a non-existent path before Push to exercise the RequireExists guard.
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		// Point RCLONE_TEST_ROOT at a path that does not exist.
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		err := a.Push(context.Background(), "")
		if err == nil {
			t.Fatal("expected error when remote root does not exist")
		}
		if !strings.Contains(err.Error(), "does not exist") {
			t.Fatalf("unexpected error: %v", err)
		}
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
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		if err := a.Push(context.Background(), ""); err == nil {
			t.Fatal("expected missing cache file error")
		}
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
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err != nil {
			t.Fatal(err)
		}
	})
}

func TestPullFailsForMissingRemotePath(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		t.Setenv("RCLONE_TEST_ROOT", filepath.Join(t.TempDir(), "nonexistent"))
		err := a.Pull(context.Background(), "", ".")
		if err == nil {
			t.Fatal("expected error when remote root does not exist")
		}
		if !strings.Contains(err.Error(), "does not exist") {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestPullRejectsHashMismatch(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	bin := filepath.Join(t.TempDir(), "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
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
  *) exit 2 ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	if err := os.MkdirAll(remoteDir, 0o755); err != nil {
		t.Fatal(err)
	}
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		cacheFile := filepath.Join(cacheDir, "files", hash.Algorithm)
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		cacheFile = filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())
		if err := os.Remove(cacheFile); err != nil {
			t.Fatal(err)
		}
		if err := a.Pull(context.Background(), "", "data/blob"); err == nil {
			t.Fatal("expected error on hash mismatch")
		}
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
		if err := a.Add(context.Background(), []string{"data/blob"}); err != nil {
			t.Fatal(err)
		}
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())); err != nil {
			t.Fatal(err)
		}
		err = a.Push(context.Background(), "")
		if !errors.Is(err, errs.ErrMissingCachedFile) {
			t.Fatalf("expected ErrMissingCachedFile, got: %v", err)
		}
	})
}

func TestAddWithCacheDirReadOnly(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	filesDir := filepath.Join(cacheDir, "files")
	if err := os.MkdirAll(filesDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(filesDir, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(filesDir, 0o755) })
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))
	inDir(t, repo, func() {
		err := app(&bytes.Buffer{}).Add(context.Background(), []string{"data/blob"})
		if err == nil {
			t.Fatal("expected error when cache files dir is read-only")
		}
		if _, statErr := os.Lstat(filepath.Join(repo, "data", "blob")); statErr != nil {
			t.Fatal("source file must still exist after failed Add")
		}
	})
}

func app(stdout *bytes.Buffer) App {
	return App{
		Stdout:     stdout,
		Stderr:     &bytes.Buffer{},
		ConfigPath: ".git-sfs/config.toml",
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	return repo
}

// writeDataset sets up a fake rclone binary that maps "localtest:" to remoteDir
// and writes a config.toml pointing at it.  Tests that inspect remoteDir/files/...
// directly still work because the fake rclone stores files there.
func writeDataset(t *testing.T, repo, remoteDir string) {
	t.Helper()
	if err := os.MkdirAll(remoteDir, 0o755); err != nil {
		t.Fatal(err)
	}
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
    if [ -f "$src" ]; then
      size=$(wc -c < "$src" | tr -d ' \t')
      printf '[{"Path":"%s","Size":%s}]\n' "$(basename "$src")" "$size"
    elif [ -e "$src" ]; then
      printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
    else
      printf 'directory not found: %s\n' "$src" >&2; exit 1
    fi ;;
  copy)
    ignore_existing=false; files_from=""; shift
    while [ "$#" -gt 2 ]; do
      case "$1" in
        --ignore-existing) ignore_existing=true; shift ;;
        --files-from) files_from="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    src_base="$1"; dst_base="$2"
    while IFS= read -r rel; do
      [ -z "$rel" ] && continue
      src_file="$(map_path "${src_base}/${rel}")"
      dst_file="$(map_path "${dst_base}/${rel}")"
      if $ignore_existing && [ -e "$dst_file" ]; then continue; fi
      mkdir -p "$(dirname "$dst_file")"
      cp "$src_file" "$dst_file"
    done < "$files_from" ;;
  *) exit 2 ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", remoteDir)
	content := "version = 1\n\n[remotes.default]\nbackend = localtest\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeRcloneDataset(t *testing.T, repo, remote, path string) {
	t.Helper()
	content := "version = 1\n\n[remotes.default]\nbackend = " + remote + "\npath = " + path + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeLocal(t *testing.T, repo, cacheDir string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(repo, ".git-sfs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(cacheDir, filepath.Join(repo, ".git-sfs", "cache")); err != nil {
		t.Fatal(err)
	}
}

func mustWrite(t *testing.T, path string, content []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, content, 0o644); err != nil {
		t.Fatal(err)
	}
}

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}

func writeTimedRcloneTool(t *testing.T, path string) {
	t.Helper()
	writeTool(t, path, `set -eu
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
cmd="${1:-}"
map_path() {
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copyto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    case "$src" in
      "$RCLONE_TEST_ROOT"/*)
        printf 'start %s\n' "$src" >> "$RCLONE_TEST_LOG"
        sleep 1
        cp "$src" "$dst"
        printf 'end %s\n' "$src" >> "$RCLONE_TEST_LOG"
        ;;
      *)
        cp "$src" "$dst"
        ;;
    esac
    ;;
  lsjson)
    src="$(map_path "$2")"
    case "$src" in
      */files/sha256/*)
        printf 'start %s\n' "$src" >> "$RCLONE_TEST_LOG"
        sleep 1
        if [ -f "$src" ]; then
          size=$(wc -c < "$src" | tr -d ' \t')
          printf '[{"Path":"%s","Size":%s}]\n' "$(basename "$src")" "$size"
        elif [ -e "$src" ]; then
          printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
        else
          printf '[]\n'
        fi
        printf 'end %s\n' "$src" >> "$RCLONE_TEST_LOG"
        ;;
      *)
        if [ -e "$src" ]; then
          printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
        else
          printf '[]\n'
        fi
        ;;
    esac
    ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  *)
    exit 2
    ;;
esac
`)
}

func assertParallelStarts(t *testing.T, logPath, label string) {
	t.Helper()
	log, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(log)), "\n")
	// Find any two consecutive "start" lines, indicating parallel execution.
	// Non-parallel preamble calls (e.g. preflight ping) may appear first.
	for i := 0; i < len(lines)-1; i++ {
		if strings.HasPrefix(lines[i], "start ") && strings.HasPrefix(lines[i+1], "start ") {
			return
		}
	}
	t.Fatalf("%s did not start in parallel:\n%s", label, log)
}

func mustCopy(t *testing.T, src, dst string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0o644); err != nil {
		t.Fatal(err)
	}
}

func inDir(t *testing.T, dir string, fn func()) {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := os.Chdir(wd); err != nil {
			t.Fatal(err)
		}
	}()
	fn()
}
