package core

import (
	"context"
	"errors"

	"git-sfs/internal/materialize"
	"git-sfs/internal/progress"
)

func (a App) Materialize(ctx context.Context, path string) error {
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	bar := progress.New(a.Stderr, "pull", len(hashes), a.Quiet)
	defer bar.Close()
	matErrs := make([]error, len(hashes))
	runIndexed(ctx, len(hashes), a.jobs(cfg, len(hashes)), func(i int) error {
		h := hashes[i]
		if err := c.Protect(h); err != nil {
			return err
		}
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
		bar.Step()
		return nil
	}, func(i int, err error) {
		matErrs[i] = err
	})
	return errors.Join(matErrs...)
}

func (a App) Dematerialize(ctx context.Context, path string) error {
	repo, _, _, err := a.open()
	if err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		if err := materialize.Unlink(repo, l.Hash); err != nil {
			return err
		}
	}
	return nil
}
