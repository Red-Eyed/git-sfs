package core

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/require"
)

func TestInitSetupAndGitignore(t *testing.T) {
	repo := newRepo(t)
	inDir(t, repo, func() {
		a := app(&bytes.Buffer{})
		require.NoError(t, a.Init(context.Background(), false))
		require.NoError(t, a.Setup(context.Background()))

		target, err := os.Readlink(filepath.Join(repo, ".git-sfs", "cache"))
		require.NoError(t, err)
		require.NotEmpty(t, target, "cache symlink missing")

		info, err := os.Stat(filepath.Join(repo, ".git-sfs", ".cache", "files"))
		require.NoError(t, err)
		require.True(t, info.IsDir(), "default cache missing")

		require.Error(t, a.Init(context.Background(), false), "init should not overwrite config")
		require.NoError(t, a.Init(context.Background(), true))

		gitignore, err := os.ReadFile(filepath.Join(repo, ".gitignore"))
		require.NoError(t, err)
		require.Contains(t, string(gitignore), ".git-sfs/cache")
	})
}
