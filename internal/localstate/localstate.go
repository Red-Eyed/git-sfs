package localstate

import (
	"fmt"
	"os"
	"path/filepath"

	"merk/internal/cache"
	"merk/internal/config"
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

// ResolveCache keeps machine-local cache paths out of .merk/config.toml.
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
	return cache.Cache{}, fmt.Errorf("cache path is not configured; use --cache, MERK_CACHE, or .merk/cache")
}

// InitMerk creates the local project state directory used by materialization.
func InitMerk(repo string) error {
	for _, p := range []string{
		filepath.Join(repo, ".merk"),
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
	if err := InitMerk(repo); err != nil {
		return err
	}
	link := filepath.Join(repo, ".merk", "cache")
	target := abs(c.Root)
	existing, err := os.Readlink(link)
	if err == nil {
		if !filepath.IsAbs(existing) {
			existing = filepath.Join(filepath.Dir(link), existing)
		}
		if filepath.Clean(existing) == filepath.Clean(target) {
			return nil
		}
		return fmt.Errorf("cache link %s points to %s, not %s", link, existing, target)
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
