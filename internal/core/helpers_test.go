package core

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

func app(stdout *bytes.Buffer) App {
	return appWithStderr(stdout, &bytes.Buffer{})
}

func appWithStderr(stdout, stderr *bytes.Buffer) App {
	return App{
		Stdout:     stdout,
		Stderr:     stderr,
		ConfigPath: ".git-sfs/config.toml",
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	return repo
}

func writeRcloneDataset(t *testing.T, repo, remote, path string) {
	t.Helper()
	content := "version = 1\n\n[remotes.default]\nbackend = " + remote + "\npath = " + path + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeLocal(t *testing.T, repo, cacheDir string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(repo, ".git-sfs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(cacheDir, filepath.Join(repo, ".git-sfs", "cache")); err != nil {
		t.Fatal(err)
	}
}

func mustWrite(t *testing.T, path string, content []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, content, 0o644); err != nil {
		t.Fatal(err)
	}
}

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}

func mustCopy(t *testing.T, src, dst string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0o644); err != nil {
		t.Fatal(err)
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
