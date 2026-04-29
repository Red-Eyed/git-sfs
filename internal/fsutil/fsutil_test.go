package fsutil

import (
	"os"
	"path/filepath"
	"testing"
)

func TestAtomicCopyPublishesCompleteFile(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	dst := filepath.Join(dir, "nested", "dst")
	if err := os.WriteFile(src, []byte("payload"), 0o640); err != nil {
		t.Fatal(err)
	}
	if err := AtomicCopy(src, dst, 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(dst)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "payload" {
		t.Fatalf("got %q", got)
	}
	info, err := os.Stat(dst)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("mode = %v", info.Mode().Perm())
	}
}

func TestSymlinkHelpers(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "target")
	link := filepath.Join(dir, "links", "link")
	if err := os.WriteFile(target, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := RelSymlink(target, link); err != nil {
		t.Fatal(err)
	}
	got, err := os.Readlink(link)
	if err != nil {
		t.Fatal(err)
	}
	if filepath.IsAbs(got) {
		t.Fatalf("relative symlink got absolute target %q", got)
	}
	if err := AbsoluteSymlink(target, link); err != nil {
		t.Fatal(err)
	}
	got, err = os.Readlink(link)
	if err != nil {
		t.Fatal(err)
	}
	if got != target {
		t.Fatalf("got %q want %q", got, target)
	}
}

func TestIsInside(t *testing.T) {
	root := filepath.Join(t.TempDir(), "root")
	if !IsInside(root, filepath.Join(root, "child")) {
		t.Fatal("child should be inside root")
	}
	if IsInside(root, root) {
		t.Fatal("root itself is not considered inside")
	}
	if IsInside(root, filepath.Dir(root)) {
		t.Fatal("parent should not be inside root")
	}
}

func TestEnsureDirWrapsPath(t *testing.T) {
	path := filepath.Join(t.TempDir(), "a", "b")
	if err := EnsureDir(path); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(path); err != nil || !info.IsDir() {
		t.Fatalf("dir not created: %v", err)
	}
}
