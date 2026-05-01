package core

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/localstate"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/progress"
)

// Init creates the tracked project config and the untracked .git-sfs workspace.
func (a App) Init(ctx context.Context, force bool) (err error) {
	a.debugf("init: start")
	defer a.debugDone("init", &err)
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return err
	}
	cfgPath := filepath.Join(repo, a.ConfigPath)
	if _, err := os.Stat(cfgPath); err == nil && !force {
		err = fmt.Errorf("%s already exists; use --force to overwrite", a.ConfigPath)
		return err
	}
	if err := localstate.InitGitSFS(repo); err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(cfgPath), 0o755); err != nil {
		return err
	}
	if err := config.WriteDefault(cfgPath); err != nil {
		return err
	}
	c := cache.Cache{Root: filepath.Join(repo, ".git-sfs", ".cache")}
	if a.CacheFlag != "" || os.Getenv("GIT_SFS_CACHE") != "" {
		var err error
		c, err = localstate.ResolveCache(repo, a.CacheFlag)
		if err != nil {
			return err
		}
	}
	if err := c.Init(); err != nil {
		return err
	}
	if err := localstate.BindCache(repo, c); err != nil {
		return err
	}
	if err := ensureGitignore(repo); err != nil {
		return err
	}
	a.say("initialized git-sfs repository")
	return nil
}

// Setup prepares machine-local state and repairs materialization links when the
// referenced cache files already exist.
func (a App) Setup(ctx context.Context) (err error) {
	a.debugf("setup: start")
	defer a.debugDone("setup", &err)
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	if err := localstate.InitGitSFS(repo); err != nil {
		return err
	}
	if err := c.Init(); err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "setup")
	if err != nil {
		return err
	}
	defer l.Release()
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	hashes := uniqueHashesFromTracked(links)
	bar := progress.New(a.Stderr, "setup", len(hashes), a.Quiet)
	defer bar.Close()
	setupErrs := make([]error, len(hashes))
	runIndexed(ctx, len(hashes), a.jobs(cfg, len(hashes)), func(i int) error {
		h := hashes[i]
		if c.HasValid(h) {
			if err := c.Protect(h); err != nil {
				return err
			}
			if err := materialize.Link(repo, c, h); err != nil {
				return err
			}
		}
		bar.Step()
		return nil
	}, func(i int, err error) {
		setupErrs[i] = err
	})
	if err := errors.Join(setupErrs...); err != nil {
		return err
	}
	a.say("setup complete")
	return nil
}

func ensureGitignore(repo string) error {
	path := filepath.Join(repo, ".gitignore")
	b, _ := os.ReadFile(path)
	seen := map[string]bool{}
	for _, line := range strings.Split(string(b), "\n") {
		seen[strings.TrimSpace(line)] = true
	}
	var missing []string
	for _, entry := range []string{".git-sfs/cache", ".git-sfs/.cache"} {
		if !seen[entry] {
			missing = append(missing, entry)
		}
	}
	if len(missing) == 0 {
		return nil
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return err
	}
	defer f.Close()
	if len(b) > 0 && !strings.HasSuffix(string(b), "\n") {
		if _, err := f.WriteString("\n"); err != nil {
			return err
		}
	}
	_, err = f.WriteString(strings.Join(missing, "\n") + "\n")
	return err
}
