package localstate

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/vadstup/merk/internal/cache"
	"github.com/vadstup/merk/internal/config"
)

// ResolveRepo walks upward from the current directory until it finds .git.
func ResolveRepo() (string, error) {
	wd, err := os.Getwd()
	if err != nil {
		return "", err
	}
	for {
		if st, err := os.Stat(filepath.Join(wd, ".git")); err == nil && st.IsDir() {
			return wd, nil
		}
		parent := filepath.Dir(wd)
		if parent == wd {
			return "", fmt.Errorf("not inside a Git repository")
		}
		wd = parent
	}
}

// ResolveCache keeps machine-local cache paths out of dataset.yaml.
func ResolveCache(repo, flagValue string) (cache.Cache, error) {
	if flagValue != "" {
		return cache.Cache{Root: abs(flagValue)}, nil
	}
	if env := os.Getenv("MERK_CACHE"); env != "" {
		return cache.Cache{Root: abs(env)}, nil
	}
	local, err := config.LoadLocal(repo)
	if err != nil {
		return cache.Cache{}, err
	}
	if local.CachePath != "" {
		return cache.Cache{Root: abs(local.CachePath)}, nil
	}
	return cache.Cache{}, fmt.Errorf("cache path is not configured; use --cache, MERK_CACHE, or .ds/local.yaml")
}

// InitDS creates the untracked local state directory used by materialization.
func InitDS(repo string) error {
	for _, p := range []string{
		filepath.Join(repo, ".ds"),
		filepath.Join(repo, ".ds", "worktree", "sha256"),
	} {
		if err := os.MkdirAll(p, 0o755); err != nil {
			return err
		}
	}
	return nil
}

func abs(path string) string {
	out, err := filepath.Abs(path)
	if err != nil {
		return path
	}
	return out
}
