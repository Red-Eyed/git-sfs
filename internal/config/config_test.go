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
	content := "version = 1\n\n[remotes.default]\nbackend = primary\npath = datasets/project\n\n[remotes.backup]\nbackend = backup\npath = dataset\nconfig = rclone.conf\n\n[settings]\nalgorithm = sha256\nn_jobs = 3\n"
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
	if cfg.Remotes["default"].Backend != "primary" || cfg.Remotes["backup"].Backend != "backup" || cfg.Remotes["backup"].Path != "dataset" || cfg.Remotes["backup"].Config != "rclone.conf" {
		t.Fatalf("unexpected remotes: %#v", cfg.Remotes)
	}
	if cfg.Settings.Algorithm != "sha256" {
		t.Fatalf("algorithm = %q", cfg.Settings.Algorithm)
	}
	if cfg.Settings.Jobs != 3 {
		t.Fatalf("n_jobs = %d", cfg.Settings.Jobs)
	}
}

func TestDefault(t *testing.T) {
	cfg := Default()
	if cfg.Version != Version || cfg.Settings.Algorithm != "sha256" || cfg.Remotes["default"].Backend == "" {
		t.Fatalf("bad default config: %#v", cfg)
	}
	if cfg.Settings.Jobs != 0 {
		t.Fatalf("default n_jobs = %d", cfg.Settings.Jobs)
	}
}

func TestLoadDatasetErrors(t *testing.T) {
	cases := map[string]string{
		"missing version":        "[settings]\nalgorithm = sha256\n",
		"bad version":            "version = 2\n[settings]\nalgorithm = sha256\n",
		"bad algorithm":          "version = 1\n[settings]\nalgorithm = md5\n",
		"bad n_jobs":             "version = 1\n[settings]\nalgorithm = sha256\nn_jobs = nope\n",
		"negative n_jobs":        "version = 1\n[settings]\nalgorithm = sha256\nn_jobs = -1\n",
		"unknown root":           "version = 1\nwat = true\n",
		"unknown settings field": "version = 1\n[settings]\nother = x\n",
		"backend missing":        "version = 1\n[remotes.default]\npath = p\n[settings]\nalgorithm = sha256\n",
		"unknown remote field":   "version = 1\n[remotes.default]\nbackend = r\npath = p\nshell = sh\n[settings]\nalgorithm = sha256\n",
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
		"backend = \"myremote\"",
		"path = \"datasets/project\"",
		"config = \"rclone.conf\"",
		"algorithm = \"sha256\"",
		"n_jobs = 0",
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

func TestParseSemver(t *testing.T) {
	cases := []struct {
		in   string
		want [3]int
		fail bool
	}{
		{"1.67.0", [3]int{1, 67, 0}, false},
		{"v1.67.0", [3]int{1, 67, 0}, false},
		{"0.0.1", [3]int{0, 0, 1}, false},
		{"1.60", [3]int{}, true},
		{"nope", [3]int{}, true},
		{"a.b.c", [3]int{}, true},
	}
	for _, tc := range cases {
		got, err := ParseSemver(tc.in)
		if tc.fail {
			if err == nil {
				t.Errorf("ParseSemver(%q): expected error", tc.in)
			}
			continue
		}
		if err != nil {
			t.Errorf("ParseSemver(%q): unexpected error: %v", tc.in, err)
			continue
		}
		if got != tc.want {
			t.Errorf("ParseSemver(%q) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

func TestCheckRcloneVersion(t *testing.T) {
	if err := CheckRcloneVersion("1.67.0", "1.60.0"); err != nil {
		t.Errorf("1.67.0 >= 1.60.0 should pass: %v", err)
	}
	if err := CheckRcloneVersion("1.60.0", "1.60.0"); err != nil {
		t.Errorf("equal versions should pass: %v", err)
	}
	if err := CheckRcloneVersion("1.59.9", "1.60.0"); err == nil {
		t.Error("1.59.9 < 1.60.0 should fail")
	}
	if err := CheckRcloneVersion("2.0.0", "1.99.9"); err != nil {
		t.Errorf("major version bump should pass: %v", err)
	}
	if err := CheckRcloneVersion("1.60.0", "nope"); err == nil {
		t.Error("malformed minimum should fail")
	}
}

func TestLoadMinRcloneVersion(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	content := "version = 1\n\n[remotes.default]\nbackend = r\npath = p\n\n[settings]\nalgorithm = sha256\nmin_rclone_version = \"1.67.0\"\n"
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Settings.MinRcloneVersion != "1.67.0" {
		t.Fatalf("min_rclone_version = %q", cfg.Settings.MinRcloneVersion)
	}
}
