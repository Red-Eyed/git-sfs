package core

import (
	"context"
	"errors"
	"fmt"

	"git-sfs/internal/cache"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/remote"
)

// Pull downloads missing files for the selected symlinks.
func (a App) Pull(ctx context.Context, path string) (err error) {
	a.debugf("pull: start path=%s", path)
	defer a.debugDone("pull", &err)
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	if err := c.Init(); err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "pull")
	if err != nil {
		return err
	}
	defer l.Release()
	r, err := a.selectRemote(repo, cfg, "")
	if err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	if err := pullMissingFiles(ctx, c, r, hashes, a.jobs(cfg, len(hashes))); err != nil {
		return err
	}
	pullErrs := make([]error, len(hashes))
	runIndexed(ctx, len(hashes), a.jobs(cfg, len(hashes)), func(i int) error {
		h := hashes[i]
		if err := c.Protect(h); err != nil {
			return err
		}
		return materialize.Link(repo, c, h)
	}, func(i int, err error) {
		pullErrs[i] = err
	})
	return errors.Join(pullErrs...)
}

func pullMissingFiles(ctx context.Context, c cache.Cache, r remote.Remote, hashes []hash.Hash, workers int) error {
	errsByIndex := make([]error, len(hashes))
	runIndexed(ctx, len(hashes), workers, func(i int) error {
		h := hashes[i]
		if c.HasValid(h) {
			return nil
		}
		if err := r.PullFile(ctx, h, c.FilePath(h)); err != nil {
			return fmt.Errorf("pull %s: %w", h, err)
		}
		return nil
	}, func(i int, err error) {
		errsByIndex[i] = err
	})
	for _, err := range errsByIndex {
		if err != nil {
			return err
		}
	}
	return nil
}
