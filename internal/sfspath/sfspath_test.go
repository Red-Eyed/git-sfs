package sfspath

import (
	"os"
	"path/filepath"
	"testing"

	"git-sfs/internal/hash"
)

func TestParseGitSymlink(t *testing.T) {
	repo := t.TempDir()
	file := filepath.Join(repo, "data", "blob")
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if err := os.MkdirAll(filepath.Dir(file), 0o755); err != nil {
		t.Fatal(err)
	}
	target, err := GitLinkTarget(repo, file, h)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, file); err != nil {
		t.Fatal(err)
	}
	got, _, err := ParseGitSymlink(repo, file)
	if err != nil {
		t.Fatal(err)
	}
	if got != h {
		t.Fatalf("got %s want %s", got, h)
	}
}

func TestRejectsAbsoluteGitSymlink(t *testing.T) {
	repo := t.TempDir()
	file := filepath.Join(repo, "data")
	if err := os.Symlink("/tmp/cache/file", file); err != nil {
		t.Fatal(err)
	}
	if _, _, err := ParseGitSymlink(repo, file); err == nil {
		t.Fatal("expected absolute symlink target to be rejected")
	}
}

func TestRejectsInvalidGitSymlinks(t *testing.T) {
	repo := t.TempDir()
	cases := map[string]string{
		"outside":       "../outside",
		"bad structure": filepath.Join(".git-sfs", "cache", "files", hash.Algorithm, "aa"),
		"bad hash":      filepath.Join(".git-sfs", "cache", "files", hash.Algorithm, "aa", "not-a-hash"),
		"bad prefix":    filepath.Join(".git-sfs", "cache", "files", hash.Algorithm, "bb", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
	}
	for name, target := range cases {
		t.Run(name, func(t *testing.T) {
			file := filepath.Join(repo, name)
			if err := os.Symlink(target, file); err != nil {
				t.Fatal(err)
			}
			if _, _, err := ParseGitSymlink(repo, file); err == nil {
				t.Fatal("expected invalid symlink error")
			}
		})
	}
}

func TestIsSFSSymlink(t *testing.T) {
	repo := t.TempDir()
	file := filepath.Join(repo, "blob")
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	target, err := GitLinkTarget(repo, file, h)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, file); err != nil {
		t.Fatal(err)
	}
	if !IsSFSSymlink(repo, file) {
		t.Fatal("expected git-sfs symlink")
	}
	regular := filepath.Join(repo, "regular")
	if err := os.WriteFile(regular, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if IsSFSSymlink(repo, regular) {
		t.Fatal("regular file should not be git-sfs symlink")
	}
}
