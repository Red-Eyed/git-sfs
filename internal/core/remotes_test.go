package core

import (
	"bytes"
	"encoding/json"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/require"
)

const twoRemotesConfig = "version = 1\n\n" +
	"[remotes.default]\nbackend = myremote\npath = datasets/project\nconfig = rclone.conf\n\n" +
	"[remotes.backup]\nbackend = s3\npath = bucket/data\n\n" +
	"[settings]\nalgorithm = sha256\n"

func TestRemotesListsConfiguredRemotes(t *testing.T) {
	repo := newRepo(t)
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(twoRemotesConfig))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		require.NoError(t, app(stdout).Remotes(false))
	})
	out := stdout.String()
	require.Contains(t, out, "remotes: 2")
	// Sorted by name: backup before default.
	require.Less(t, indexOf(out, "backup:"), indexOf(out, "default:"))
	require.Contains(t, out, "backup: backend=s3 path=bucket/data")
	require.Contains(t, out, "default: backend=myremote path=datasets/project config=rclone.conf (default)")
}

func TestRemotesJSON(t *testing.T) {
	repo := newRepo(t)
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(twoRemotesConfig))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		require.NoError(t, app(stdout).Remotes(true))
	})

	var report struct {
		Remotes []struct {
			Name    string `json:"name"`
			Backend string `json:"backend"`
			Path    string `json:"path"`
			Config  string `json:"config"`
			Default bool   `json:"default"`
		} `json:"remotes"`
	}
	require.NoError(t, json.Unmarshal(stdout.Bytes(), &report))
	require.Len(t, report.Remotes, 2)
	require.Equal(t, "backup", report.Remotes[0].Name)
	require.False(t, report.Remotes[0].Default)
	require.Equal(t, "default", report.Remotes[1].Name)
	require.True(t, report.Remotes[1].Default)
	require.Equal(t, "myremote", report.Remotes[1].Backend)
}

func indexOf(s, sub string) int {
	return bytes.Index([]byte(s), []byte(sub))
}
