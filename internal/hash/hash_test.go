package hash

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestFileRespectsCanceledContext(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "data")
	if err := os.WriteFile(path, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := File(ctx, path); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

func TestFileHashChangesWithContent(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "data")
	if err := os.WriteFile(path, []byte("one"), 0o644); err != nil {
		t.Fatal(err)
	}
	h1, err := File(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("two"), 0o644); err != nil {
		t.Fatal(err)
	}
	h2, err := File(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	if h1 == h2 {
		t.Fatalf("hash did not change: %s", h1)
	}
}

func TestVerifyFileAndParse(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "data")
	if err := os.WriteFile(path, []byte("data"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := File(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	if err := VerifyFile(context.Background(), path, h); err != nil {
		t.Fatal(err)
	}
	parsed, err := Parse(h.String())
	if err != nil {
		t.Fatal(err)
	}
	if parsed != h {
		t.Fatalf("got %s want %s", parsed, h)
	}
	if h.Prefix() != h.String()[:2] {
		t.Fatalf("bad prefix %q", h.Prefix())
	}
}

func TestVerifyFileRejectsMismatch(t *testing.T) {
	path := filepath.Join(t.TempDir(), "data")
	if err := os.WriteFile(path, []byte("data"), 0o644); err != nil {
		t.Fatal(err)
	}
	bad := Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if err := VerifyFile(context.Background(), path, bad); err == nil {
		t.Fatal("expected mismatch")
	}
}

func TestParseRejectsInvalidHashes(t *testing.T) {
	for _, input := range []string{"abc", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg"} {
		if _, err := Parse(input); err == nil {
			t.Fatalf("expected %q to fail", input)
		}
	}
}

func TestFileMissingAndShortPrefix(t *testing.T) {
	if _, err := File(context.Background(), filepath.Join(t.TempDir(), "missing")); err == nil {
		t.Fatal("expected missing file error")
	}
	if Hash("a").Prefix() != "" {
		t.Fatal("short hash should not have prefix")
	}
}
