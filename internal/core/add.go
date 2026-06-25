package core

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"git-sfs/internal/cache"
	"git-sfs/internal/hash"
	"git-sfs/internal/localstate"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/progress"
	"git-sfs/internal/sfspath"
)

type addPrepared struct {
	Hash hash.Hash
	Err  error
}

// Add converts regular files into git-sfs symlinks after copying their bytes into
// the local content-addressed cache.
func (a App) Add(ctx context.Context, paths []string) (err error) {
	a.debugf("add: start paths=%d", len(paths))
	defer a.debugDone("add", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	repo, c, cfg := s.repo, s.cache, s.cfg
	if err := localstate.InitGitSFS(repo); err != nil {
		return err
	}
	if err := c.Init(); err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "add")
	if err != nil {
		return err
	}
	defer l.Release()
	var files []string
	for _, p := range paths {
		root := absFromRepo(repo, p)
		if err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if shouldSkip(repo, path) {
				if d.IsDir() {
					return filepath.SkipDir
				}
				return nil
			}
			if d.Type().IsRegular() {
				files = append(files, path)
			}
			return nil
		}); err != nil {
			return err
		}
	}
	sort.Strings(files)
	// Byte-weighted progress: the bar tracks bytes hashed (the first of three
	// passes; copy and verification follow). This gives smooth within-file
	// progress for large files instead of a count that jumps 0 -> 100%.
	bar := progress.NewBytes(a.Stderr, "add", sumFileSizes(files), a.Quiet)
	defer bar.Close()
	prepared := prepareAddFiles(ctx, c, files, a.jobs(cfg, len(files)), bar.Add)
	for i, file := range files {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if prepared[i].Err != nil {
			return prepared[i].Err
		}
		h := prepared[i].Hash
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
		target, err := sfspath.GitLinkTarget(repo, file, h)
		if err != nil {
			return err
		}
		if err := os.Remove(file); err != nil {
			return err
		}
		if err := os.Symlink(target, file); err != nil {
			return err
		}
		a.say("added " + rel(repo, file) + " -> " + h.String())
	}
	return nil
}

func prepareAddFiles(ctx context.Context, c cache.Cache, files []string, workers int, onRead func(n int)) []addPrepared {
	out := make([]addPrepared, len(files))
	runIndexed(ctx, len(files), workers, func(i int) error {
		h, err := hash.FileProgress(ctx, files[i], onRead)
		if err != nil {
			return err
		}
		if err := c.Store(ctx, files[i], h); err != nil {
			return fmt.Errorf("store %s: %w", files[i], err)
		}
		out[i].Hash = h
		return nil
	}, func(i int, err error) {
		out[i].Err = err
	})
	return out
}
