package core

import (
	"context"
	"fmt"
	"sync"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
	"git-sfs/internal/progress"
)

// Push uploads each referenced cache file at most once per invocation.
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
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	bar := progress.New(a.Stderr, "push", len(hashes), a.Quiet)
	defer bar.Close()
	workers := a.jobs(cfg, len(hashes))
	jobs := make(chan hash.Hash)
	errCh := make(chan error, 1)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var once sync.Once
	var wg sync.WaitGroup
	var outMu sync.Mutex
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for h := range jobs {
				if ctx.Err() != nil {
					return
				}
				if !c.HasValid(h) {
					once.Do(func() {
						errCh <- fmt.Errorf("%w: %s", errs.ErrMissingCachedFile, h)
						cancel()
					})
					return
				}
				has, err := r.HasFile(ctx, h)
				if err != nil {
					once.Do(func() {
						errCh <- err
						cancel()
					})
					return
				}
				if has {
					bar.Step()
					continue
				}
				if err := r.PushFile(ctx, h, c.FilePath(h)); err != nil {
					once.Do(func() {
						errCh <- err
						cancel()
					})
					return
				}
				outMu.Lock()
				a.say("pushed " + h.String())
				outMu.Unlock()
				bar.Step()
			}
		}()
	}
	go func() {
		defer close(jobs)
		for _, h := range hashes {
			select {
			case <-ctx.Done():
				return
			case jobs <- h:
			}
		}
	}()
	wg.Wait()
	select {
	case err := <-errCh:
		return err
	default:
		return nil
	}
}
