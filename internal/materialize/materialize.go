package materialize

import (
	"fmt"
	"os"

	"merk/internal/cache"
	"merk/internal/fsutil"
	"merk/internal/hash"
	"merk/internal/merkpath"
)

// Link creates or repairs the untracked .ds/worktree symlink for h.
func Link(repo string, c cache.Cache, h hash.Hash) error {
	obj := c.FilePath(h)
	if _, err := os.Stat(obj); err != nil {
		return fmt.Errorf("cache file missing for %s: %w", h, err)
	}
	link := merkpath.WorktreeFile(repo, h)
	if existing, err := os.Readlink(link); err == nil && existing == obj {
		return nil
	}
	return fsutil.AbsoluteSymlink(obj, link)
}

// Unlink removes only the local materialization hop, never the cached file.
func Unlink(repo string, h hash.Hash) error {
	err := os.Remove(merkpath.WorktreeFile(repo, h))
	if os.IsNotExist(err) {
		return nil
	}
	return err
}
