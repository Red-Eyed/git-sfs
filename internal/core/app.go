package core

import (
	"context"
	"fmt"
	"io"
	"path/filepath"
	"runtime"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/localstate"
	"git-sfs/internal/remote"
)

type App struct {
	Stdout     io.Writer
	Stderr     io.Writer
	CacheFlag  string
	ConfigPath string
	Jobs       int
	Quiet      bool
	Verbose    bool
}

func (a App) open() (string, cache.Cache, config.Config, error) {
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return "", cache.Cache{}, config.Config{}, err
	}
	cfg, err := config.Load(filepath.Join(repo, a.ConfigPath))
	if err != nil {
		return "", cache.Cache{}, config.Config{}, err
	}
	c, err := localstate.ResolveCache(repo, a.CacheFlag)
	if err != nil {
		return "", cache.Cache{}, config.Config{}, err
	}
	if err := localstate.BindCache(repo, c); err != nil {
		return "", cache.Cache{}, config.Config{}, err
	}
	return repo, c, cfg, nil
}

func (a App) selectRemote(repo string, cfg config.Config, name string) (remote.Remote, error) {
	if name == "" {
		name = "default"
	}
	rc, ok := cfg.Remotes[name]
	if !ok {
		return nil, fmt.Errorf("remote %q is not configured", name)
	}
	var debug io.Writer
	if a.Verbose {
		debug = a.Stderr
	}
	return remote.NewWithOptions(rc, remote.Options{Debug: debug, ConfigDir: filepath.Dir(filepath.Join(repo, a.ConfigPath))})
}

func (a App) jobs(cfg config.Config, n int) int {
	if a.Jobs > 0 {
		return jobsFromSettings(a.Jobs, n)
	}
	return jobsFromSettings(cfg.Settings.Jobs, n)
}

func jobsFromSettings(configured, n int) int {
	if n < 1 {
		return 1
	}
	if configured <= 0 {
		configured = runtime.GOMAXPROCS(0)
		if configured > 4 {
			configured = 4
		}
	}
	if configured > n {
		configured = n
	}
	if configured < 1 {
		return 1
	}
	return configured
}

func (a App) say(s string) {
	if !a.Quiet {
		fmt.Fprintln(a.Stdout, s)
	}
}

func (a App) debugf(format string, args ...any) {
	if !a.Verbose {
		return
	}
	fmt.Fprintf(a.Stderr, "debug: "+format+"\n", args...)
}

// preflight checks that rclone is on PATH and the remote is reachable.
// Call this after selectRemote, before starting any transfer.
func (a App) preflight(ctx context.Context, r remote.Remote) error {
	if err := remote.CheckRcloneOnPath(); err != nil {
		return err
	}
	return r.Ping(ctx)
}

func (a App) debugDone(name string, err *error) {
	if !a.Verbose {
		return
	}
	if err != nil && *err != nil {
		a.debugf("%s: error: %v", name, *err)
		return
	}
	a.debugf("%s: done", name)
}
