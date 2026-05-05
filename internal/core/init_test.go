package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInitSetupAndGitignore(t *testing.T) {
	repo := newRepo(t)
	inDir(t, repo, func() {
		stdout := &bytes.Buffer{}
		a := app(stdout)
		if err := a.Init(context.Background(), false); err != nil {
			t.Fatal(err)
		}
		if err := a.Setup(context.Background()); err != nil {
			t.Fatal(err)
		}
		if target, err := os.Readlink(filepath.Join(repo, ".git-sfs", "cache")); err != nil || target == "" {
			t.Fatalf("cache symlink missing: target=%q err=%v", target, err)
		}
		if info, err := os.Stat(filepath.Join(repo, ".git-sfs", ".cache", "files")); err != nil || !info.IsDir() {
			t.Fatalf("default cache missing: %v", err)
		}
		if err := a.Init(context.Background(), false); err == nil {
			t.Fatal("init should not overwrite config")
		}
		if err := a.Init(context.Background(), true); err != nil {
			t.Fatal(err)
		}
		gitignore, err := os.ReadFile(filepath.Join(repo, ".gitignore"))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(gitignore), ".git-sfs/cache") {
			t.Fatalf(".gitignore missing .git-sfs/: %q", gitignore)
		}
	})
}
