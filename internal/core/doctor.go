package core

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"

	"git-sfs/internal/config"
	"git-sfs/internal/localstate"
	"git-sfs/internal/remote"
	"git-sfs/internal/version"
)

// Doctor runs a series of configuration and connectivity checks.
// If all checks pass, push, pull, and verify are expected to work correctly.
// remoteName filters to a single remote; empty string checks all remotes.
func (a App) Doctor(ctx context.Context, remoteName string) error {
	d := &doctorRun{w: a.Stdout}
	fmt.Fprintln(a.Stdout)

	// 1. Git repository
	repo, repoOK := d.check("git repository", func() (string, error) {
		r, err := localstate.ResolveRepo()
		if err != nil {
			return "", err
		}
		return r, nil
	})
	if !repoOK {
		d.skipAll("git-sfs config", "git-sfs version", "cache config", "cache directory",
			"rclone binary", "rclone version")
		return d.done(a.Stdout)
	}

	// 2. git-sfs config
	configPath := filepath.Join(repo, a.ConfigPath)
	var cfg config.Config
	_, cfgOK := d.check("git-sfs config", func() (string, error) {
		c, err := config.Load(configPath)
		if err != nil {
			return "", err
		}
		cfg = c
		return configPath, nil
	})
	if !cfgOK {
		d.skipAll("git-sfs version", "cache config", "cache directory",
			"rclone binary", "rclone version")
		return d.done(a.Stdout)
	}

	// 3. git-sfs version
	d.check("git-sfs version", func() (string, error) {
		ver := version.Version
		if min := cfg.Settings.MinGitSFSVersion; min != "" {
			if err := config.CheckGitSFSVersion(ver, min); err != nil {
				return "", err
			}
			return fmt.Sprintf("%s (min: %s)", ver, min), nil
		}
		return ver, nil
	})

	// 4. Cache config
	cacheRoot := ""
	_, cacheOK := d.check("cache config", func() (string, error) {
		c, err := localstate.ResolveCache(repo, a.CacheFlag)
		if err != nil {
			return "", err
		}
		cacheRoot = c.Root
		return c.Root, nil
	})

	// 5. Cache directory
	if cacheOK {
		d.check("cache directory", func() (string, error) {
			if _, err := os.Stat(cacheRoot); os.IsNotExist(err) {
				return "", fmt.Errorf("does not exist: %s (run git-sfs setup or create it)", cacheRoot)
			}
			tmp, err := os.CreateTemp(cacheRoot, ".git-sfs-doctor-*")
			if err != nil {
				return "", fmt.Errorf("not writable: %w", err)
			}
			tmp.Close()
			os.Remove(tmp.Name())
			return fmt.Sprintf("%s (writable)", cacheRoot), nil
		})
	} else {
		d.skipAll("cache directory")
	}

	// 6. rclone binary
	_, rcloneOK := d.check("rclone binary", func() (string, error) {
		p, err := exec.LookPath("rclone")
		if err != nil {
			return "", fmt.Errorf("rclone not found on PATH: install from https://rclone.org/downloads/")
		}
		return p, nil
	})
	if !rcloneOK {
		d.skipAll("rclone version")
		return d.done(a.Stdout)
	}

	// 7. rclone version (binary-level, config-independent)
	d.check("rclone version", func() (string, error) {
		ver, err := remote.DetectRcloneVersion(ctx, "")
		if err != nil {
			return "", err
		}
		if min := cfg.Settings.MinRcloneVersion; min != "" {
			if err := config.CheckRcloneVersion(ver, min); err != nil {
				return "", err
			}
			return fmt.Sprintf("v%s (min: %s)", ver, min), nil
		}
		return "v" + ver, nil
	})

	// Per-remote checks.
	configDir := filepath.Dir(configPath)
	remoteNames := remoteCheckList(cfg, remoteName)
	for _, name := range remoteNames {
		fmt.Fprintf(a.Stdout, "\n  [remote: %s]\n", name)
		a.checkRemote(ctx, d, cfg, configDir, name)
	}

	return d.done(a.Stdout)
}

// checkRemote runs connectivity checks for a single configured remote.
func (a App) checkRemote(ctx context.Context, d *doctorRun, cfg config.Config, configDir, name string) {
	rc, ok := cfg.Remotes[name]
	if !ok {
		d.fail("config", fmt.Errorf("remote %q is not defined in config", name))
		return
	}

	rcloneConf := remote.ResolveConfigPath(configDir, rc.Config)

	// rclone config file (per-remote)
	if rcloneConf != "" {
		d.check("rclone config file", func() (string, error) {
			if _, err := os.Stat(rcloneConf); err != nil {
				return "", fmt.Errorf("file not found: %s", rcloneConf)
			}
			return rcloneConf, nil
		})
	} else {
		d.info("rclone config file", "using rclone default (~/.config/rclone/rclone.conf)")
	}

	var debug io.Writer
	if a.Verbose {
		debug = a.Stderr
	}
	r, err := remote.NewWithOptions(rc, remote.Options{Debug: debug, ConfigDir: configDir, RetryMax: 1})
	if err != nil {
		d.fail("remote backend", err)
		d.skipAll("remote path")
		return
	}

	// backend reachable
	_, backendOK := d.check("remote backend", func() (string, error) {
		if err := r.CheckBackend(ctx); err != nil {
			return "", err
		}
		return rc.Backend + ":", nil
	})
	if !backendOK {
		d.skipAll("remote path")
		return
	}

	// remote path exists
	d.check("remote path", func() (string, error) {
		if err := r.CheckPath(ctx); err != nil {
			return "", err
		}
		url := rc.Backend + ":"
		if rc.Path != "" {
			url += rc.Path
		}
		return url, nil
	})
}

// remoteCheckList returns the list of remote names to check.
// If filter is non-empty, only that remote is returned.
// Otherwise all configured remotes are returned in sorted order,
// with "default" first if present.
func remoteCheckList(cfg config.Config, filter string) []string {
	if filter != "" {
		return []string{filter}
	}
	names := make([]string, 0, len(cfg.Remotes))
	for name := range cfg.Remotes {
		names = append(names, name)
	}
	sort.Slice(names, func(i, j int) bool {
		if names[i] == "default" {
			return true
		}
		if names[j] == "default" {
			return false
		}
		return names[i] < names[j]
	})
	return names
}

// doctorRun accumulates check results and formats the output.
type doctorRun struct {
	w       io.Writer
	passed  int
	failed  int
	skipped int
}

func (d *doctorRun) check(label string, fn func() (string, error)) (string, bool) {
	detail, err := fn()
	if err != nil {
		d.failed++
		fmt.Fprintf(d.w, "  %-24s FAIL: %v\n", label+":", err)
		return "", false
	}
	d.passed++
	if detail != "" {
		fmt.Fprintf(d.w, "  %-24s ok  (%s)\n", label+":", detail)
	} else {
		fmt.Fprintf(d.w, "  %-24s ok\n", label+":")
	}
	return detail, true
}

func (d *doctorRun) info(label, detail string) {
	d.passed++
	fmt.Fprintf(d.w, "  %-24s ok  (%s)\n", label+":", detail)
}

func (d *doctorRun) fail(label string, err error) {
	d.failed++
	fmt.Fprintf(d.w, "  %-24s FAIL: %v\n", label+":", err)
}

func (d *doctorRun) skip(label string) {
	d.skipped++
	fmt.Fprintf(d.w, "  %-24s skip\n", label+":")
}

func (d *doctorRun) skipAll(labels ...string) {
	for _, label := range labels {
		d.skip(label)
	}
}

func (d *doctorRun) done(w io.Writer) error {
	fmt.Fprintln(w)
	if d.failed == 0 && d.skipped == 0 {
		fmt.Fprintf(w, "doctor: all %d checks passed\n", d.passed)
		return nil
	}
	if d.failed == 0 {
		fmt.Fprintf(w, "doctor: %d passed, %d skipped\n", d.passed, d.skipped)
		return nil
	}
	fmt.Fprintf(w, "doctor: %d passed, %d failed, %d skipped\n", d.passed, d.failed, d.skipped)
	return fmt.Errorf("doctor: %d check(s) failed", d.failed)
}
