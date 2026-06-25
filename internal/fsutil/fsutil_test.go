package fsutil

import (
	"context"
	"errors"
	"io"
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
	if err := AtomicCopy(context.Background(), src, dst, 0o600); err != nil {
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

func TestAtomicCopyRespectsCanceledContext(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	dst := filepath.Join(dir, "dst")
	if err := os.WriteFile(src, []byte("payload"), 0o640); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := AtomicCopy(ctx, src, dst, 0o600)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
	if _, err := os.Stat(dst); !os.IsNotExist(err) {
		t.Fatalf("destination must not be published on cancel: %v", err)
	}
}

// endlessReader never returns EOF, so CopyCtx would loop forever unless it
// honors cancellation. It cancels the context once enough bytes have been read.
type endlessReader struct {
	read     int
	cancelAt int
	cancel   func()
}

func (r *endlessReader) Read(p []byte) (int, error) {
	r.read += len(p)
	if r.read >= r.cancelAt {
		r.cancel()
	}
	return len(p), nil
}

func TestCopyCtxStopsMidStreamOnCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	src := &endlessReader{cancelAt: 64, cancel: cancel}
	written, err := CopyCtx(ctx, io.Discard, src, make([]byte, 16))
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
	// The loop must stop shortly after cancellation, not copy unboundedly.
	if written > 16*8 {
		t.Fatalf("copied %d bytes after cancel; expected it to stop promptly", written)
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
