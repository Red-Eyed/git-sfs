package cache

import (
	"os"
	"path/filepath"
	"testing"

	"merk/internal/hash"
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
	if err := os.WriteFile(c.FilePath(h), []byte("corrupt"), 0o644); err != nil {
		t.Fatal(err)
	}
	if c.HasValid(h) {
		t.Fatal("corrupt file should not be valid")
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
