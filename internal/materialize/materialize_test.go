package materialize

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/vadstup/merk/internal/cache"
	"github.com/vadstup/merk/internal/hash"
	"github.com/vadstup/merk/internal/merkpath"
)

func TestLinkAndUnlink(t *testing.T) {
	repo := t.TempDir()
	c := cache.Cache{Root: filepath.Join(t.TempDir(), "cache")}
	if err := c.Init(); err != nil {
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
	target, err := os.Readlink(merkpath.WorktreeFile(repo, h))
	if err != nil {
		t.Fatal(err)
	}
	if target != c.FilePath(h) {
		t.Fatalf("got %q want %q", target, c.FilePath(h))
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
