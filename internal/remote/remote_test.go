package remote

import (
	"testing"

	"git-sfs/internal/config"
)

func TestNewRemote(t *testing.T) {
	cases := []config.RemoteConfig{
		{Type: "filesystem", URL: t.TempDir()},
		{Type: "fs", URL: t.TempDir()},
		{Type: "rclone", URL: "remote:path"},
		{Type: "rclone", Host: "remote", Path: "D:/data"},
	}
	for _, tc := range cases {
		if _, err := New(tc); err != nil {
			t.Fatalf("%#v: %v", tc, err)
		}
	}
	if _, err := New(config.RemoteConfig{Type: "wat", URL: "x"}); err == nil {
		t.Fatal("expected unsupported remote error")
	}
	if _, err := New(config.RemoteConfig{Type: "rsync", URL: "x"}); err == nil {
		t.Fatal("expected rsync to be unsupported")
	}
	if _, err := New(config.RemoteConfig{Type: "ssh", URL: "x"}); err == nil {
		t.Fatal("expected ssh to be unsupported")
	}
}

func TestRcloneConfigPath(t *testing.T) {
	r, err := NewWithOptions(
		config.RemoteConfig{Type: "rclone", Host: "remote", Path: "dataset", Config: "rclone.conf"},
		Options{ConfigDir: "/repo/.git-sfs"},
	)
	if err != nil {
		t.Fatal(err)
	}
	got := r.(rcloneRemote).config
	if got != "/repo/.git-sfs/rclone.conf" {
		t.Fatalf("bad rclone config path %q", got)
	}
}
