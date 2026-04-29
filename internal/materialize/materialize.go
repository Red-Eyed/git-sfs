package materialize

import (
	"fmt"
	"os"

	"github.com/vadstup/merk/internal/cache"
	"github.com/vadstup/merk/internal/fsutil"
	"github.com/vadstup/merk/internal/hash"
	"github.com/vadstup/merk/internal/merkpath"
)

// Link creates or repairs the untracked .ds/worktree symlink for h.
func Link(repo string, c cache.Cache, h hash.Hash) error {
	obj := c.ObjectPath(h)
	if _, err := os.Stat(obj); err != nil {
		return fmt.Errorf("cache object missing for %s: %w", h, err)
	}
	link := merkpath.WorktreeObject(repo, h)
	if existing, err := os.Readlink(link); err == nil && existing == obj {
		return nil
	}
	return fsutil.AbsoluteSymlink(obj, link)
}

// Unlink removes only the local materialization hop, never the cached object.
func Unlink(repo string, h hash.Hash) error {
	err := os.Remove(merkpath.WorktreeObject(repo, h))
	if os.IsNotExist(err) {
		return nil
	}
	return err
}
