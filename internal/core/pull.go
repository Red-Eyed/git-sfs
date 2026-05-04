package core

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"syscall"

	"git-sfs/internal/cache"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/remote"
)

// Pull downloads missing files for the selected symlinks.
func (a App) Pull(ctx context.Context, remoteName, path string) (err error) {
	a.debugf("pull: start remote=%s path=%s", remoteName, path)
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
	r, err := a.selectRemote(repo, cfg, remoteName)
	if err != nil {
		return err
	}
	if err := a.preflight(ctx, r); err != nil {
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
	var missing []hash.Hash
	for _, h := range hashes {
		if !c.HasValid(h) {
			missing = append(missing, h)
		}
	}
	if len(missing) == 0 {
		return nil
	}
	if err := checkDiskSpace(ctx, c, r, missing, workers); err != nil {
		return err
	}
	// Remove any corrupt/partial local files so --ignore-existing doesn't skip them.
	for _, h := range missing {
		p := c.FilePath(h)
		if _, err := os.Stat(p); err == nil {
			os.Remove(p)
		}
	}
	relPaths := make([]string, len(missing))
	for i, h := range missing {
		relPaths[i] = hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
	}
	return r.CopyFromRemote(ctx, filepath.Join(c.Root, "files"), relPaths)
}

// checkDiskSpace estimates total bytes needed for the missing hashes and fails
// if the cache volume has less than 110% of that available.
func checkDiskSpace(ctx context.Context, c cache.Cache, r remote.Remote, missing []hash.Hash, workers int) error {
	var total atomic.Int64
	var mu sync.Mutex
	var firstErr error
	errs := make([]error, len(missing))
	runIndexed(ctx, len(missing), workers, func(i int) error {
		size, err := r.FileSize(ctx, missing[i])
		if err != nil {
			return err
		}
		if size > 0 {
			total.Add(size)
		}
		return nil
	}, func(i int, err error) {
		mu.Lock()
		if firstErr == nil {
			firstErr = err
		}
		errs[i] = err
		mu.Unlock()
	})
	if firstErr != nil {
		return firstErr
	}
	needed := total.Load()
	if needed <= 0 {
		return nil
	}
	avail, err := availableBytes(c.Root)
	if err != nil {
		return nil // skip guard if we can't stat
	}
	// Require at least 110% of needed bytes to leave a safety margin.
	if uint64(needed)*110/100 > avail {
		return fmt.Errorf("insufficient disk space: need ~%d bytes, have %d available in %s", needed, avail, c.Root)
	}
	return nil
}

// availableBytes returns the number of bytes available to non-root processes on
// the filesystem containing path.
func availableBytes(path string) (uint64, error) {
	var fs syscall.Statfs_t
	if err := syscall.Statfs(path, &fs); err != nil {
		return 0, err
	}
	return fs.Bavail * uint64(fs.Bsize), nil
}
