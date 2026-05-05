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
