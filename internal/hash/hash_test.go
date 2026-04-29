package hash

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFileHashChangesWithContent(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "data")
	if err := os.WriteFile(path, []byte("one"), 0o644); err != nil {
		t.Fatal(err)
	}
	h1, err := File(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("two"), 0o644); err != nil {
		t.Fatal(err)
	}
	h2, err := File(path)
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
	h, err := File(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := VerifyFile(path, h); err != nil {
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
	if err := VerifyFile(path, bad); err == nil {
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
	if _, err := File(filepath.Join(t.TempDir(), "missing")); err == nil {
		t.Fatal("expected missing file error")
	}
	if Hash("a").Prefix() != "" {
		t.Fatal("short hash should not have prefix")
	}
}
