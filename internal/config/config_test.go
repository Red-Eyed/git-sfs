package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDatasetRejectsCachePath(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dataset.yaml")
	if err := os.WriteFile(path, []byte("version: 1\ncache:\n  path: /tmp/cache\nsettings:\n  algorithm: sha256\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err == nil {
		t.Fatal("expected cache path to be rejected")
	}
}

func TestLoadDataset(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dataset.yaml")
	content := "version: 1\n\nremotes:\n  default:\n    type: filesystem\n    url: /tmp/remote\n  backup:\n    type: rsync\n    url: host:/remote\n\nsettings:\n  algorithm: sha256\n"
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
	if cfg.Remotes["default"].Type != "filesystem" || cfg.Remotes["backup"].URL != "host:/remote" {
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
		"missing version":        "settings:\n  algorithm: sha256\n",
		"bad version":            "version: 2\nsettings:\n  algorithm: sha256\n",
		"bad algorithm":          "version: 1\nsettings:\n  algorithm: md5\n",
		"unknown root":           "version: 1\nwat: true\n",
		"unknown settings field": "version: 1\nsettings:\n  other: x\n",
		"remote missing url":     "version: 1\nremotes:\n  default:\n    type: filesystem\nsettings:\n  algorithm: sha256\n",
	}
	for name, content := range cases {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "dataset.yaml")
			if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
				t.Fatal(err)
			}
			if _, err := Load(path); err == nil {
				t.Fatal("expected error")
			}
		})
	}
}

func TestWriteDefault(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dataset.yaml")
	if err := WriteDefault(path); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err != nil {
		t.Fatal(err)
	}
}

func TestLoadLocal(t *testing.T) {
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".ds"), 0o755); err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(repo, "cache")
	if err := os.WriteFile(filepath.Join(repo, ".ds", "local.yaml"), []byte("cache:\n  path: "+want+"\n"), 0o644); err != nil {
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
