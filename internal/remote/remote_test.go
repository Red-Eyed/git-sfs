package remote

import (
	"testing"

	"git-sfs/internal/config"
)

func TestNewRemote(t *testing.T) {
	cases := []config.RemoteConfig{
		{Type: "filesystem", URL: t.TempDir()},
		{Type: "fs", URL: t.TempDir()},
		{Type: "rsync", URL: t.TempDir()},
		{Type: "ssh", URL: t.TempDir()},
		{Type: "rclone", URL: "remote:path"},
	}
	for _, tc := range cases {
		if _, err := New(tc); err != nil {
			t.Fatalf("%#v: %v", tc, err)
		}
	}
	if _, err := New(config.RemoteConfig{Type: "wat", URL: "x"}); err == nil {
		t.Fatal("expected unsupported remote error")
	}
}
