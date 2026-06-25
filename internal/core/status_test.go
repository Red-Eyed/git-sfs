package core

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/require"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

// statusReportJSON mirrors the status command's JSON envelope for assertions.
type statusReportJSON struct {
	Tracked       int   `json:"tracked"`
	UniqueFiles   int   `json:"unique_files"`
	Cached        int   `json:"cached"`
	MissingLocal  int   `json:"missing_local"`
	TotalSize     int64 `json:"total_size"`
	RemoteChecked bool  `json:"remote_checked"`
	OnRemote      *int  `json:"on_remote"`
	Unpushed      *int  `json:"unpushed"`
	Files         []struct {
		Path   string `json:"path"`
		Hash   string `json:"hash"`
		Size   int64  `json:"size"`
		Cached bool   `json:"cached"`
		Remote *bool  `json:"remote"`
	} `json:"files"`
}

func TestStatusLocalOnlyReportsSizesWithoutRemote(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload-bytes"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		stdout.Reset()
		// checkRemote=false must not touch the remote at all.
		require.NoError(t, app(stdout).Status(context.Background(), "", false, "."))
	})
	out := stdout.String()
	require.Contains(t, out, "tracked symlinks: 1")
	require.Contains(t, out, "cached locally: 1")
	require.Contains(t, out, "missing locally: 0")
	require.Contains(t, out, "total size: 13 B")
	require.Contains(t, out, "data/blob: 13 B cached")
	require.NotContains(t, out, "on remote:")
}

func TestStatusSizesMissingLocalFileFromRemote(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	remoteDir := filepath.Join(t.TempDir(), "remote")
	writeDataset(t, repo, remoteDir)
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload-bytes"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Push(context.Background(), ""))
		h, _, err := sfspath.ParseGitSymlink(repo, filepath.Join(repo, "data", "blob"))
		require.NoError(t, err)
		// Drop the local cache file: size must now come from the remote, no download.
		require.NoError(t, os.Remove(filepath.Join(cacheDir, "files", hash.Algorithm, h.Prefix(), h.String())))

		stdout.Reset()
		require.NoError(t, app(stdout).Status(context.Background(), "default", false, "."))
	})
	out := stdout.String()
	require.Contains(t, out, "cached locally: 0")
	require.Contains(t, out, "missing locally: 1")
	require.Contains(t, out, "on remote: 1")
	require.Contains(t, out, "unpushed: 0")
	require.Contains(t, out, "total size: 13 B")
	require.Contains(t, out, "data/blob: 13 B missing on-remote")
}

func TestStatusReportsUnpushedFile(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("never-pushed"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		stdout.Reset()
		require.NoError(t, app(stdout).Status(context.Background(), "default", false, "."))
	})
	out := stdout.String()
	require.Contains(t, out, "on remote: 0")
	require.Contains(t, out, "unpushed: 1")
	require.Contains(t, out, "unpushed")
}

func TestStatusJSONIncludesRemoteWhenChecked(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		require.NoError(t, a.Push(context.Background(), ""))
		stdout.Reset()
		require.NoError(t, app(stdout).Status(context.Background(), "default", true, "."))
	})

	var report statusReportJSON
	require.NoError(t, json.Unmarshal(stdout.Bytes(), &report))
	require.Equal(t, 1, report.Tracked)
	require.Equal(t, 1, report.UniqueFiles)
	require.Equal(t, 1, report.Cached)
	require.True(t, report.RemoteChecked)
	require.NotNil(t, report.OnRemote)
	require.Equal(t, 1, *report.OnRemote)
	require.Len(t, report.Files, 1)
	require.Equal(t, "data/blob", report.Files[0].Path)
	require.Equal(t, int64(7), report.Files[0].Size)
	require.NotNil(t, report.Files[0].Remote)
	require.True(t, *report.Files[0].Remote)
}

func TestStatusJSONOmitsRemoteWhenLocalOnly(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "blob"), []byte("payload"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data/blob"}))
		stdout.Reset()
		require.NoError(t, app(stdout).Status(context.Background(), "", true, "."))
	})

	var report statusReportJSON
	require.NoError(t, json.Unmarshal(stdout.Bytes(), &report))
	require.False(t, report.RemoteChecked)
	require.Nil(t, report.OnRemote)
	require.Nil(t, report.Unpushed)
	require.Len(t, report.Files, 1)
	require.Nil(t, report.Files[0].Remote)
}

func TestStatusPathScopesToSubtree(t *testing.T) {
	repo := newRepo(t)
	cacheDir := filepath.Join(t.TempDir(), "cache")
	writeDataset(t, repo, filepath.Join(t.TempDir(), "remote"))
	writeLocal(t, repo, cacheDir)
	mustWrite(t, filepath.Join(repo, "data", "one.bin"), []byte("one"))
	mustWrite(t, filepath.Join(repo, "data", "nested", "two.bin"), []byte("two"))

	stdout := &bytes.Buffer{}
	inDir(t, repo, func() {
		a := app(stdout)
		require.NoError(t, a.Add(context.Background(), []string{"data"}))
		stdout.Reset()
		require.NoError(t, app(stdout).Status(context.Background(), "", false, "data/nested"))
	})
	out := stdout.String()
	require.Contains(t, out, "tracked symlinks: 1")
	require.Contains(t, out, "data/nested/two.bin")
	require.NotContains(t, out, "data/one.bin")
}
