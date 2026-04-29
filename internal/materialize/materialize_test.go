package materialize

import (
	"os"
	"path/filepath"
	"testing"

	"merk/internal/cache"
	"merk/internal/hash"
)

func TestLinkAndUnlink(t *testing.T) {
	repo := t.TempDir()
	c := cache.Cache{Root: filepath.Join(t.TempDir(), "cache")}
	if err := c.Init(); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(repo, ".merk"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(c.Root, filepath.Join(repo, ".merk", "cache")); err != nil {
		t.Fatal(err)
	}
	src := filepath.Join(t.TempDir(), "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := c.Store(src, h); err != nil {
		t.Fatal(err)
	}
	if err := Link(repo, c, h); err != nil {
		t.Fatal(err)
	}
	if err := Link(repo, c, h); err != nil {
		t.Fatal(err)
	}
	if err := Unlink(repo, h); err != nil {
		t.Fatal(err)
	}
	if err := Unlink(repo, h); err != nil {
		t.Fatal(err)
	}
}

func TestLinkFailsWhenFileMissing(t *testing.T) {
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if err := Link(t.TempDir(), cache.Cache{Root: t.TempDir()}, h); err == nil {
		t.Fatal("expected missing file error")
	}
}
