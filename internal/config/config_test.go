package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDatasetRejectsCachePath(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	if err := os.WriteFile(path, []byte("version = 1\n\n[cache]\npath = /tmp/cache\n\n[settings]\nalgorithm = sha256\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err == nil {
		t.Fatal("expected cache path to be rejected")
	}
}

func TestLoadDataset(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	content := "version = 1\n\n[remotes.default]\ntype = filesystem\npath = /tmp/remote\n\n[remotes.backup]\ntype = rclone\nhost = remote\npath = dataset\nconfig = rclone.conf\n\n[settings]\nalgorithm = sha256\n"
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Version != Version {
		t.Fatalf("version = %d", cfg.Version)
	}
	if cfg.Remotes["default"].Type != "filesystem" || cfg.Remotes["backup"].Host != "remote" || cfg.Remotes["backup"].Path != "dataset" || cfg.Remotes["backup"].Config != "rclone.conf" {
		t.Fatalf("unexpected remotes: %#v", cfg.Remotes)
	}
	if cfg.Settings.Algorithm != "sha256" {
		t.Fatalf("algorithm = %q", cfg.Settings.Algorithm)
	}
}

func TestDefault(t *testing.T) {
	cfg := Default()
	if cfg.Version != Version || cfg.Settings.Algorithm != "sha256" || cfg.Remotes["default"].Type == "" {
		t.Fatalf("bad default config: %#v", cfg)
	}
}

func TestLoadDatasetErrors(t *testing.T) {
	cases := map[string]string{
		"missing version":        "[settings]\nalgorithm = sha256\n",
		"bad version":            "version = 2\n[settings]\nalgorithm = sha256\n",
		"bad algorithm":          "version = 1\n[settings]\nalgorithm = md5\n",
		"unknown root":           "version = 1\nwat = true\n",
		"unknown settings field": "version = 1\n[settings]\nother = x\n",
		"remote missing path":    "version = 1\n[remotes.default]\ntype = filesystem\n[settings]\nalgorithm = sha256\n",
		"unknown remote field":   "version = 1\n[remotes.default]\ntype = rclone\nhost = h\npath = p\nshell = sh\n[settings]\nalgorithm = sha256\n",
	}
	for name, content := range cases {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "config.toml")
			if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
				t.Fatal(err)
			}
			if _, err := Load(path); err == nil {
				t.Fatal("expected error")
			}
		})
	}
}

func TestWriteDefaultCreatesEditableStarterConfig(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	if err := WriteDefault(path); err != nil {
		t.Fatal(err)
	}
	content, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	text := string(content)
	for _, want := range []string{
		"# git-sfs project config",
		"type = \"rclone\"",
		"host = \"remote-name\"",
		"path = \"datasets/project\"",
		"config = \"rclone.conf\"",
		"# type = \"filesystem\"",
		"algorithm = \"sha256\"",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("default config missing %q:\n%s", want, text)
		}
	}
}

func TestWriteDefault(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	if err := WriteDefault(path); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err != nil {
		t.Fatal(err)
	}
}

func TestLoadLocal(t *testing.T) {
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".git-sfs"), 0o755); err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(repo, "cache")
	if err := os.Symlink(want, filepath.Join(repo, ".git-sfs", "cache")); err != nil {
		t.Fatal(err)
	}
	local, err := LoadLocal(repo)
	if err != nil {
		t.Fatal(err)
	}
	if local.CachePath != want {
		t.Fatalf("got %q want %q", local.CachePath, want)
	}
}

func TestLoadLocalMissingIsEmpty(t *testing.T) {
	local, err := LoadLocal(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if local.CachePath != "" {
		t.Fatalf("unexpected cache path %q", local.CachePath)
	}
}
