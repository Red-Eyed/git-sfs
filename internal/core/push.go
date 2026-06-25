package core

import (
	"context"
	"fmt"
	"path/filepath"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
)

// Push uploads each referenced cache file to the remote in a single rclone call.
// Existing remote files are never overwritten.
func (a App) Push(ctx context.Context, name string) (err error) {
	a.debugf("push: start remote=%s", name)
	defer a.debugDone("push", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	repo, c, cfg := s.repo, s.cache, s.cfg
	l, err := lock.Acquire(ctx, c.LocksDir(), "push")
	if err != nil {
		return err
	}
	defer l.Release()
	r, err := a.selectRemote(s, name)
	if err != nil {
		return err
	}
	if err := a.preflight(ctx, cfg, r); err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	relPaths := make([]string, 0, len(hashes))
	for _, h := range hashes {
		if !c.HasValid(ctx, h) {
			return fmt.Errorf("%w: %s", errs.ErrMissingCachedFile, h)
		}
		relPaths = append(relPaths, hash.Algorithm+"/"+h.Prefix()+"/"+h.String())
	}
	a.debugf("push: uploading %d file(s)", len(relPaths))
	return r.CopyToRemote(ctx, filepath.Join(c.Root, "files"), relPaths)
}
