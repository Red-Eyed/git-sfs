package materialize

import (
	"fmt"
	"os"

	"merk/internal/cache"
	"merk/internal/hash"
	"merk/internal/merkpath"
)

// Link verifies that the repo-local .merk/cache path reaches the cached file.
func Link(repo string, c cache.Cache, h hash.Hash) error {
	file := c.FilePath(h)
	if _, err := os.Stat(file); err != nil {
		return fmt.Errorf("cache file missing for %s: %w", h, err)
	}
	if _, err := os.Stat(merkpath.CacheLinkFile(repo, h)); err != nil {
		return fmt.Errorf("cache link missing for %s: %w", h, err)
	}
	return nil
}

// Unlink is kept for the dematerialize command; direct cache symlinks have no per-file hop.
func Unlink(repo string, h hash.Hash) error {
	return nil
}
