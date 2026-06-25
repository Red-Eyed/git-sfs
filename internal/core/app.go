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
	"git-sfs/internal/version"
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

// resolvedConfigPath returns ConfigPath as-is when it is absolute, or joined
// with repo when it is relative. filepath.Join cannot be used directly because
// it does not treat an absolute second argument specially on all platforms.
func (a App) resolvedConfigPath(repo string) string {
	if filepath.IsAbs(a.ConfigPath) {
		return a.ConfigPath
	}
	return filepath.Join(repo, a.ConfigPath)
}

// session is the resolved working context every command operates on: the repo
// root, its bound cache, and the loaded config. open() returns it as one value
// so callers (and selectRemote) take a single argument instead of threading the
// three separately.
type session struct {
	repo  string
	cache cache.Cache
	cfg   config.Config
}

func (a App) open() (session, error) {
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return session{}, err
	}
	cfg, err := config.Load(a.resolvedConfigPath(repo))
	if err != nil {
		return session{}, err
	}
	if min := cfg.Settings.MinGitSFSVersion; min != "" {
		if err := config.CheckGitSFSVersion(version.Version, min); err != nil {
			return session{}, err
		}
	}
	c, err := localstate.ResolveCache(repo, a.CacheFlag)
	if err != nil {
		return session{}, err
	}
	if err := localstate.BindCache(repo, c); err != nil {
		return session{}, err
	}
	return session{repo: repo, cache: c, cfg: cfg}, nil
}

func (a App) selectRemote(s session, name string) (remote.Remote, error) {
	if name == "" {
		name = "default"
	}
	rc, ok := s.cfg.Remotes[name]
	if !ok {
		return nil, fmt.Errorf("remote %q is not configured", name)
	}
	var debug io.Writer
	if a.Verbose {
		debug = a.Stderr
	}
	// Let rclone render its own transfer progress bar to stderr unless the user
	// silenced output; this covers push/pull, where rclone moves the bytes.
	var progress io.Writer
	if !a.Quiet {
		progress = a.Stderr
	}
	return remote.NewWithOptions(rc, remote.Options{
		Debug:     debug,
		Progress:  progress,
		ConfigDir: filepath.Dir(a.resolvedConfigPath(s.repo)),
		RetryMax:  s.cfg.Settings.RetryMax,
	})
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

// preflight checks that rclone is on PATH, meets the minimum version
// requirement (if configured), and that the remote root exists.
// Call this after selectRemote, before starting any transfer.
func (a App) preflight(ctx context.Context, cfg config.Config, r remote.Remote) error {
	if err := remote.CheckRcloneOnPath(); err != nil {
		return err
	}
	if min := cfg.Settings.MinRcloneVersion; min != "" {
		ver, err := remote.DetectRcloneVersion(ctx, "")
		if err != nil {
			return fmt.Errorf("check rclone version: %w", err)
		}
		if err := config.CheckRcloneVersion(ver, min); err != nil {
			return err
		}
	}
	return r.RequireExists(ctx)
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
