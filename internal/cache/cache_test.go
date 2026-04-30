package cache

import (
	"os"
	"path/filepath"
	"testing"

	"git-sfs/internal/hash"
)

func TestStoreUsesContentAddressedPathAndDetectsCorruption(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	c := Cache{Root: filepath.Join(dir, "cache")}
	if err := c.Init(); err != nil {
		t.Fatal(err)
	}
	if err := c.Store(src, h); err != nil {
		t.Fatal(err)
	}
	if got := c.FilePath(h); filepath.Base(filepath.Dir(got)) != h.Prefix() {
		t.Fatalf("file path %q does not include hash prefix %q", got, h.Prefix())
	}
	if !c.HasValid(h) {
		t.Fatal("stored file should be valid")
	}
	info, err := os.Stat(c.FilePath(h))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("stored file should be read-only, got %v", info.Mode().Perm())
	}
	if err := os.Chmod(c.FilePath(h), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(c.FilePath(h), []byte("corrupt"), 0o644); err != nil {
		t.Fatal(err)
	}
	if c.HasValid(h) {
		t.Fatal("corrupt file should not be valid")
	}
}

func TestMovePlacesSourceAtContentAddressedPath(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	c := Cache{Root: filepath.Join(dir, "cache")}
	if err := c.Init(); err != nil {
		t.Fatal(err)
	}
	if err := c.Move(src, h); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("source should be gone after move: %v", err)
	}
	if !c.HasValid(h) {
		t.Fatal("moved file should be valid in cache")
	}
	info, err := os.Stat(c.FilePath(h))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("moved file should be read-only, got %v", info.Mode().Perm())
	}
}

func TestMoveReusesExistingObject(t *testing.T) {
	dir := t.TempDir()
	first := filepath.Join(dir, "first")
	second := filepath.Join(dir, "second")
	if err := os.WriteFile(first, []byte("same"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(second, []byte("same"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(first)
	if err != nil {
		t.Fatal(err)
	}
	c := Cache{Root: filepath.Join(dir, "cache")}
	if err := c.Init(); err != nil {
		t.Fatal(err)
	}
	if err := c.Move(first, h); err != nil {
		t.Fatal(err)
	}
	if err := c.Move(second, h); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(second); !os.IsNotExist(err) {
		t.Fatalf("duplicate source should be removed after cache hit: %v", err)
	}
	if !c.HasValid(h) {
		t.Fatal("cache object should remain valid")
	}
}

func TestCopyThenRemoveStagesCrossFilesystemMoveFallback(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	dst := filepath.Join(dir, "cache", "tmp", "dst")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := copyThenRemove(src, dst, 0o444); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(src); !os.IsNotExist(err) {
		t.Fatalf("source should be removed after fallback copy: %v", err)
	}
	if b, err := os.ReadFile(dst); err != nil || string(b) != "payload" {
		t.Fatalf("fallback copy wrote %q err=%v", b, err)
	}
	info, err := os.Stat(dst)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("fallback copy should publish read-only staging file, got %v", info.Mode().Perm())
	}
}

func TestCacheErrors(t *testing.T) {
	dir := t.TempDir()
	fileRoot := filepath.Join(dir, "file")
	if err := os.WriteFile(fileRoot, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := (Cache{Root: fileRoot}).Init(); err == nil {
		t.Fatal("expected init error when cache root is a file")
	}
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if err := (Cache{Root: filepath.Join(dir, "cache")}).Store(filepath.Join(dir, "missing"), h); err == nil {
		t.Fatal("expected missing source error")
	}
}
