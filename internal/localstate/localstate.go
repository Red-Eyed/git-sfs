package localstate

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/errs"
)

// ResolveRepo walks upward from the current directory until it finds .git
// (directory for a regular repo, file for a submodule).
func ResolveRepo() (string, error) {
	wd, err := os.Getwd()
	if err != nil {
		return "", err
	}
	start := wd
	for {
		if _, err := os.Stat(filepath.Join(wd, ".git")); err == nil {
			return wd, nil
		}
		parent := filepath.Dir(wd)
		if parent == wd {
			return "", fmt.Errorf("not a git repository: no .git found in %s or any parent directory", start)
		}
		wd = parent
	}
}

// ResolveCache keeps machine-local cache paths out of .git-sfs/config.toml.
func ResolveCache(repo, flagValue string) (cache.Cache, error) {
	if flagValue != "" {
		return cache.Cache{Root: abs(flagValue)}, nil
	}
	if env := os.Getenv("GIT_SFS_CACHE"); env != "" {
		return cache.Cache{Root: abs(env)}, nil
	}
	local, err := config.LoadLocal(repo)
	if err != nil {
		return cache.Cache{}, err
	}
	if local.CachePath != "" {
		return cache.Cache{Root: abs(local.CachePath)}, nil
	}
	return cache.Cache{}, fmt.Errorf("%w: set GIT_SFS_CACHE, pass --cache, or run git-sfs setup", errs.ErrMissingCacheConfig)
}

// InitGitSFS creates the local project state directory used by materialization.
func InitGitSFS(repo string) error {
	for _, p := range []string{
		filepath.Join(repo, ".git-sfs"),
	} {
		if err := os.MkdirAll(p, 0o755); err != nil {
			return err
		}
	}
	return nil
}

func BindCache(repo string, c cache.Cache) error {
	if c.Root == "" {
		return nil
	}
	if err := InitGitSFS(repo); err != nil {
		return err
	}
	link := filepath.Join(repo, ".git-sfs", "cache")
	target := canonicalPath(abs(c.Root))
	existing, err := os.Readlink(link)
	if err == nil {
		if !filepath.IsAbs(existing) {
			existing = filepath.Join(filepath.Dir(link), existing)
		}
		if canonicalPath(existing) == target {
			return nil
		}
		return errors.Join(errs.ErrInvalidConfig, fmt.Errorf("cache link %s points to %s, not %s", link, existing, target))
	}
	if err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("read cache link %s: %w", link, err)
	}
	return os.Symlink(target, link)
}

func abs(path string) string {
	out, err := filepath.Abs(path)
	if err != nil {
		return path
	}
	return out
}

func canonicalPath(path string) string {
	out, err := filepath.EvalSymlinks(path)
	if err != nil {
		return filepath.Clean(path)
	}
	return out
}
