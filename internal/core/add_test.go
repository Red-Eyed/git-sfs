package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

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
