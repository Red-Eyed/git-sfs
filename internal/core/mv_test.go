package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"

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
		if err := a.Add(context.Background(), []string{"data/blob.bin"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Mv("data/blob.bin", "nested/sub/blob.bin"); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(filepath.Join(repo, "data", "blob.bin")); !os.IsNotExist(err) {
		t.Fatal("source symlink should be gone")
	}
	dst := filepath.Join(repo, "nested", "sub", "blob.bin")
	h, _, err := sfspath.ParseGitSymlink(repo, dst)
	if err != nil {
		t.Fatalf("destination is not a valid git-sfs symlink: %v", err)
	}
	if err := hash.VerifyFile(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String()), h); err != nil {
		t.Fatal(err)
	}
	// Verify the symlink resolves through the cache indirection.
	if got, _ := os.ReadFile(dst); string(got) != "content" {
		t.Fatalf("resolved content mismatch: %q", got)
	}
}

func TestMvIntoDirectory(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob.bin"), []byte("content"))
	if err := os.MkdirAll(filepath.Join(repo, "dest"), 0o755); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob.bin"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Mv("data/blob.bin", "dest"); err != nil {
			t.Fatal(err)
		}
	})
	dst := filepath.Join(repo, "dest", "blob.bin")
	if _, _, err := sfspath.ParseGitSymlink(repo, dst); err != nil {
		t.Fatalf("dest/blob.bin is not a valid git-sfs symlink: %v", err)
	}
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
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		if err := a.Mv("data", "archive"); err != nil {
			t.Fatal(err)
		}
	})
	if _, err := os.Lstat(filepath.Join(repo, "data")); !os.IsNotExist(err) {
		t.Fatal("source directory should be removed after mv")
	}
	for _, rel := range []string{"archive/one.bin", "archive/sub/two.bin"} {
		dst := filepath.Join(repo, rel)
		if _, _, err := sfspath.ParseGitSymlink(repo, dst); err != nil {
			t.Fatalf("%s is not a valid git-sfs symlink: %v", rel, err)
		}
		if got, _ := os.ReadFile(dst); len(got) == 0 {
			t.Fatalf("%s: empty content after mv", rel)
		}
	}
}

func TestMvDirectoryIntoExisting(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	if err := os.MkdirAll(filepath.Join(repo, "archive"), 0o755); err != nil {
		t.Fatal(err)
	}
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		// dst exists as directory → POSIX: place src inside it
		if err := a.Mv("data", "archive"); err != nil {
			t.Fatal(err)
		}
	})
	dst := filepath.Join(repo, "archive", "data", "one.bin")
	if _, _, err := sfspath.ParseGitSymlink(repo, dst); err != nil {
		t.Fatalf("archive/data/one.bin is not a valid git-sfs symlink: %v", err)
	}
}

func TestMvWorksOnBrokenSymlinks(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob.bin"), []byte("content"))
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		if err := a.Add(context.Background(), []string{"data/blob.bin"}); err != nil {
			t.Fatal(err)
		}
		// Remove all cache files — symlink becomes dangling.
		if err := os.RemoveAll(filepath.Join(cacheDir, "files")); err != nil {
			t.Fatal(err)
		}
		// mv must still work: it operates on symlink entries, not cache files.
		if err := a.Mv("data/blob.bin", "archive/blob.bin"); err != nil {
			t.Fatalf("mv on broken symlink: %v", err)
		}
	})
	if _, err := os.Lstat(filepath.Join(repo, "data", "blob.bin")); !os.IsNotExist(err) {
		t.Fatal("source symlink should be gone")
	}
	dst := filepath.Join(repo, "archive", "blob.bin")
	if _, _, err := sfspath.ParseGitSymlink(repo, dst); err != nil {
		t.Fatalf("destination is not a valid git-sfs symlink: %v", err)
	}
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
		if err := a.Add(context.Background(), []string{"data"}); err != nil {
			t.Fatal(err)
		}
		// Wipe cache to make all symlinks dangling.
		if err := os.RemoveAll(filepath.Join(cacheDir, "files")); err != nil {
			t.Fatal(err)
		}
		if err := a.Mv("data", "archive"); err != nil {
			t.Fatalf("mv dir on broken symlinks: %v", err)
		}
	})
	if _, err := os.Lstat(filepath.Join(repo, "data")); !os.IsNotExist(err) {
		t.Fatal("source directory should be removed after mv")
	}
	for _, relPath := range []string{"archive/a.bin", "archive/sub/b.bin"} {
		dst := filepath.Join(repo, relPath)
		if _, _, err := sfspath.ParseGitSymlink(repo, dst); err != nil {
			t.Fatalf("%s is not a valid git-sfs symlink: %v", relPath, err)
		}
	}
}

func TestMvRejectsNonSymlink(t *testing.T) {
	repo := newRepo(t)
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, filepath.Join(t.TempDir(), "cache"))
	mustWrite(t, filepath.Join(repo, "data", "plain.txt"), []byte("hello"))
	inDir(t, repo, func() {
		if err := app(&bytes.Buffer{}).Mv("data/plain.txt", "data/other.txt"); err == nil {
			t.Fatal("expected error moving a non-symlink")
		}
	})
}
