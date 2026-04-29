package localstate

import (
	"os"
	"path/filepath"
	"testing"
)

func TestResolveRepoWalksUp(t *testing.T) {
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	nested := filepath.Join(repo, "a", "b")
	if err := os.MkdirAll(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	inDir(t, nested, func() {
		got, err := ResolveRepo()
		if err != nil {
			t.Fatal(err)
		}
		realGot, err := filepath.EvalSymlinks(got)
		if err != nil {
			t.Fatal(err)
		}
		realRepo, err := filepath.EvalSymlinks(repo)
		if err != nil {
			t.Fatal(err)
		}
		if realGot != realRepo {
			t.Fatalf("got %q want %q", got, repo)
		}
	})
}

func TestResolveRepoFailsOutsideRepo(t *testing.T) {
	inDir(t, t.TempDir(), func() {
		if _, err := ResolveRepo(); err == nil {
			t.Fatal("expected error outside repo")
		}
	})
}

func TestResolveCachePriority(t *testing.T) {
	repo := t.TempDir()
	t.Setenv("MERK_CACHE", filepath.Join(repo, "env-cache"))
	c, err := ResolveCache(repo, filepath.Join(repo, "flag-cache"))
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(c.Root) != "flag-cache" {
		t.Fatalf("flag did not win: %q", c.Root)
	}
	c, err = ResolveCache(repo, "")
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(c.Root) != "env-cache" {
		t.Fatalf("env did not win: %q", c.Root)
	}
	t.Setenv("MERK_CACHE", "")
	if err := os.MkdirAll(filepath.Join(repo, ".merk"), 0o755); err != nil {
		t.Fatal(err)
	}
	local := filepath.Join(repo, "local-cache")
	if err := os.Symlink(local, filepath.Join(repo, ".merk", "cache")); err != nil {
		t.Fatal(err)
	}
	c, err = ResolveCache(repo, "")
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(c.Root) != "local-cache" {
		t.Fatalf("local config did not win: %q", c.Root)
	}
}

func TestResolveCacheMissing(t *testing.T) {
	t.Setenv("MERK_CACHE", "")
	if _, err := ResolveCache(t.TempDir(), ""); err == nil {
		t.Fatal("expected missing cache error")
	}
}

func TestInitMerk(t *testing.T) {
	repo := t.TempDir()
	if err := InitMerk(repo); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(filepath.Join(repo, ".merk")); err != nil || !info.IsDir() {
		t.Fatalf(".merk dir missing: %v", err)
	}
}

func inDir(t *testing.T, dir string, fn func()) {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := os.Chdir(wd); err != nil {
			t.Fatal(err)
		}
	}()
	fn()
}
