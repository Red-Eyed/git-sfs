package remote

import (
	"testing"

	"git-sfs/internal/config"
)

func TestNewRemote(t *testing.T) {
	cases := []config.RemoteConfig{
		{Backend: "myremote", Path: "path"},
		{Backend: "myremote", Path: "D:/data"},
		{Backend: "myremote"},
		{Backend: "", Path: "/abs/path"},
	}
	for _, tc := range cases {
		if _, err := New(tc); err != nil {
			t.Fatalf("%#v: %v", tc, err)
		}
	}
}

func TestRcloneConfigPath(t *testing.T) {
	r, err := NewWithOptions(
		config.RemoteConfig{Backend: "myremote", Path: "dataset", Config: "rclone.conf"},
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
