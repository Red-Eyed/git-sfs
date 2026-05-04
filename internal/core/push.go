package core

import (
	"context"
	"fmt"
	"path/filepath"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
	"git-sfs/internal/remote"
)

// Push uploads each referenced cache file to the remote in a single rclone call.
// Existing remote files are never overwritten.
func (a App) Push(ctx context.Context, name string) (err error) {
	a.debugf("push: start remote=%s", name)
	defer a.debugDone("push", &err)
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "push")
	if err != nil {
		return err
	}
	defer l.Release()
	r, err := a.selectRemote(repo, cfg, name)
	if err != nil {
		return err
	}
	if err := remote.CheckRcloneOnPath(); err != nil {
		return err
	}
	if err := r.RequireExists(ctx); err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	relPaths := make([]string, 0, len(hashes))
	for _, h := range hashes {
		if !c.HasValid(h) {
			return fmt.Errorf("%w: %s", errs.ErrMissingCachedFile, h)
		}
		relPaths = append(relPaths, hash.Algorithm+"/"+h.Prefix()+"/"+h.String())
	}
	a.debugf("push: uploading %d file(s)", len(relPaths))
	return r.CopyToRemote(ctx, filepath.Join(c.Root, "files"), relPaths)
}
