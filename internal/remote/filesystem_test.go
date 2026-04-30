package remote

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"git-sfs/internal/hash"
)

func TestFilesystemRemotePushPullVerifiesHashes(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	r := NewFilesystem(filepath.Join(dir, "remote"))
	has, err := r.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if has {
		t.Fatal("remote should start empty")
	}
	if err := r.PushFile(ctx, h, src); err != nil {
		t.Fatal(err)
	}
	has, err = r.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if !has {
		t.Fatal("remote should have pushed file")
	}
	remoteFile := filepath.Join(dir, "remote", "files", hash.Algorithm, h.Prefix(), h.String())
	info, err := os.Stat(remoteFile)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("remote file should be read-only, got %v", info.Mode().Perm())
	}
	dst := filepath.Join(dir, "dst")
	if err := r.PullFile(ctx, h, dst); err != nil {
		t.Fatal(err)
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		t.Fatal(err)
	}
	info, err = os.Stat(dst)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o222 != 0 {
		t.Fatalf("pulled file should be read-only, got %v", info.Mode().Perm())
	}
	if err := os.Chmod(remoteFile, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "remote", "files", hash.Algorithm, h.Prefix(), h.String()), []byte("bad"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := r.PullFile(ctx, h, filepath.Join(dir, "bad-dst")); err == nil {
		t.Fatal("expected corrupt remote file to be rejected")
	}
}

func TestFilesystemRemoteContextCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	r := NewFilesystem(t.TempDir())
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if _, err := r.HasFile(ctx, h); err == nil {
		t.Fatal("expected canceled has")
	}
	if err := r.PushFile(ctx, h, filepath.Join(t.TempDir(), "src")); err == nil {
		t.Fatal("expected canceled push")
	}
	if err := r.PullFile(ctx, h, filepath.Join(t.TempDir(), "dst")); err == nil {
		t.Fatal("expected canceled pull")
	}
}

func TestFilesystemRemotePushSkipsExistingValidFile(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	r := NewFilesystem(filepath.Join(dir, "remote"))
	if err := r.PushFile(ctx, h, src); err != nil {
		t.Fatal(err)
	}
	if err := r.PushFile(ctx, h, filepath.Join(dir, "missing")); err != nil {
		t.Fatal(err)
	}
}
