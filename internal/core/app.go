package core

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/vadstup/merk/internal/cache"
	"github.com/vadstup/merk/internal/config"
	"github.com/vadstup/merk/internal/hash"
	"github.com/vadstup/merk/internal/localstate"
	"github.com/vadstup/merk/internal/lock"
	"github.com/vadstup/merk/internal/materialize"
	"github.com/vadstup/merk/internal/merkpath"
	"github.com/vadstup/merk/internal/remote"
)

type App struct {
	Stdout     io.Writer
	Stderr     io.Writer
	CacheFlag  string
	ConfigPath string
	Quiet      bool
	Verbose    bool
}

type GCOptions struct {
	DryRun       bool
	WorktreeOnly bool
	Files        bool
}

// Init creates the tracked project config and the untracked .ds workspace.
func (a App) Init(ctx context.Context, force bool) error {
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return err
	}
	cfgPath := filepath.Join(repo, a.ConfigPath)
	if _, err := os.Stat(cfgPath); err == nil && !force {
		return fmt.Errorf("%s already exists; use --force to overwrite", a.ConfigPath)
	}
	if err := config.WriteDefault(cfgPath); err != nil {
		return err
	}
	if err := localstate.InitDS(repo); err != nil {
		return err
	}
	if err := ensureGitignore(repo); err != nil {
		return err
	}
	a.say("initialized merk repository")
	return nil
}

// Setup prepares machine-local state and repairs materialization links when the
// referenced cache files already exist.
func (a App) Setup(ctx context.Context) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	if err := localstate.InitDS(repo); err != nil {
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
	links, err := collectMerkSymlinks(repo, ".")
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if c.HasValid(h) {
			if err := materialize.Link(repo, c, h); err != nil {
				return err
			}
		}
	}
	a.say("setup complete")
	return nil
}

// Add converts regular files into merk symlinks after copying their bytes into
// the local content-addressed cache.
func (a App) Add(ctx context.Context, paths []string) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	if err := localstate.InitDS(repo); err != nil {
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
	for _, file := range files {
		h, err := hash.File(file)
		if err != nil {
			return err
		}
		if err := c.Store(file, h); err != nil {
			return fmt.Errorf("store %s: %w", file, err)
		}
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
		target, err := merkpath.GitLinkTarget(repo, file, h)
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

// Status reports user-actionable problems without mutating repository state.
func (a App) Status(ctx context.Context) error {
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	links, problems, err := scan(repo, c)
	if err != nil {
		return err
	}
	fmt.Fprintf(a.Stdout, "tracked symlinks: %d\n", len(links))
	for _, p := range problems {
		fmt.Fprintln(a.Stdout, p)
	}
	if _, ok := cfg.Remotes["default"]; !ok {
		fmt.Fprintln(a.Stdout, "invalid config: missing default remote")
		problems = append(problems, "missing default remote")
	}
	if len(problems) > 0 {
		return fmt.Errorf("status found %d problem(s)", len(problems))
	}
	return nil
}

// Verify is the CI-oriented strict check; any reported problem is a failure.
func (a App) Verify(ctx context.Context) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	_, problems, err := scan(repo, c)
	if err != nil {
		return err
	}
	if len(problems) > 0 {
		for _, p := range problems {
			fmt.Fprintln(a.Stdout, p)
		}
		return fmt.Errorf("verify failed with %d problem(s)", len(problems))
	}
	a.say("verify ok")
	return nil
}

func (a App) Materialize(ctx context.Context, path string) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	links, err := collectMerkSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
	}
	return nil
}

func (a App) Dematerialize(ctx context.Context, path string) error {
	repo, _, _, err := a.open()
	if err != nil {
		return err
	}
	links, err := collectMerkSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if err := materialize.Unlink(repo, h); err != nil {
			return err
		}
	}
	return nil
}

// Push uploads each referenced cache file at most once per invocation.
func (a App) Push(ctx context.Context, name string) error {
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "push")
	if err != nil {
		return err
	}
	defer l.Release()
	r, err := selectRemote(cfg, name)
	if err != nil {
		return err
	}
	links, err := collectMerkSymlinks(repo, ".")
	if err != nil {
		return err
	}
	seen := map[hash.Hash]bool{}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if seen[h] {
			continue
		}
		seen[h] = true
		if !c.HasValid(h) {
			return fmt.Errorf("cache file for %s is missing or corrupt", h)
		}
		has, err := r.HasFile(ctx, h)
		if err != nil {
			return err
		}
		if has {
			continue
		}
		if err := r.PushFile(ctx, h, c.FilePath(h)); err != nil {
			return err
		}
		a.say("pushed " + h.String())
	}
	return nil
}

// Pull downloads missing files for the selected symlinks and then restores
// the local .ds/worktree links that make those symlinks usable.
func (a App) Pull(ctx context.Context, path string) error {
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
	r, err := selectRemote(cfg, "")
	if err != nil {
		return err
	}
	links, err := collectMerkSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if !c.HasValid(h) {
			dst := c.FilePath(h)
			if err := r.PullFile(ctx, h, dst); err != nil {
				return fmt.Errorf("pull %s: %w", h, err)
			}
		}
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
	}
	return nil
}

// GC removes only data that is not referenced by the current Git symlink tree.
func (a App) GC(ctx context.Context, opts GCOptions) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	l, err := lock.Acquire(ctx, c.LocksDir(), "gc")
	if err != nil {
		return err
	}
	defer l.Release()
	if !opts.WorktreeOnly && !opts.Files {
		opts.WorktreeOnly = true
	}
	links, err := collectMerkSymlinks(repo, ".")
	if err != nil {
		return err
	}
	live := map[string]bool{}
	for _, l := range links {
		h, _, err := merkpath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		live[h.String()] = true
	}
	if opts.WorktreeOnly {
		root := filepath.Join(repo, ".ds", "worktree", hash.Algorithm)
		if err := removeUnreferenced(root, live, opts.DryRun, a.Stdout); err != nil {
			return err
		}
	}
	if opts.Files {
		root := filepath.Join(c.Root, "files", hash.Algorithm)
		if err := removeUnreferenced(root, live, opts.DryRun, a.Stdout); err != nil {
			return err
		}
	}
	return nil
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
	return repo, c, cfg, nil
}

func scan(repo string, c cache.Cache) ([]string, []string, error) {
	var links []string
	var problems []string
	err := filepath.WalkDir(repo, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if shouldSkip(repo, path) {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		info, err := os.Lstat(path)
		if err != nil {
			return err
		}
		if info.Mode()&os.ModeSymlink == 0 {
			if info.Mode().IsRegular() {
				problems = append(problems, "not converted: "+rel(repo, path))
			}
			return nil
		}
		h, _, err := merkpath.ParseGitSymlink(repo, path)
		if err != nil {
			problems = append(problems, "broken git symlink: "+rel(repo, path)+": "+err.Error())
			return nil
		}
		links = append(links, path)
		if !c.HasValid(h) {
			problems = append(problems, "missing or corrupt cache file: "+h.String())
			return nil
		}
		work := merkpath.WorktreeFile(repo, h)
		target, err := os.Readlink(work)
		if err != nil {
			problems = append(problems, "missing .ds/worktree symlink: "+h.String())
			return nil
		}
		if target != c.FilePath(h) {
			problems = append(problems, "stale .ds/worktree symlink: "+h.String())
		}
		return nil
	})
	return links, problems, err
}

func collectMerkSymlinks(repo, path string) ([]string, error) {
	root := absFromRepo(repo, path)
	var out []string
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if shouldSkip(repo, path) {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		info, err := os.Lstat(path)
		if err != nil {
			return err
		}
		if info.Mode()&os.ModeSymlink != 0 && merkpath.IsMerkSymlink(repo, path) {
			out = append(out, path)
		}
		return nil
	})
	sort.Strings(out)
	return out, err
}

func selectRemote(cfg config.Config, name string) (remote.Remote, error) {
	if name == "" {
		name = "default"
	}
	rc, ok := cfg.Remotes[name]
	if !ok {
		return nil, fmt.Errorf("remote %q is not configured", name)
	}
	return remote.New(rc)
}

func ensureGitignore(repo string) error {
	path := filepath.Join(repo, ".gitignore")
	b, _ := os.ReadFile(path)
	for _, line := range strings.Split(string(b), "\n") {
		if strings.TrimSpace(line) == ".ds/" || strings.TrimSpace(line) == ".ds" {
			return nil
		}
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
	_, err = f.WriteString(".ds/\n")
	return err
}

func removeUnreferenced(root string, live map[string]bool, dry bool, w io.Writer) error {
	return filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if os.IsNotExist(err) {
			return nil
		}
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		if live[filepath.Base(path)] {
			return nil
		}
		if dry {
			fmt.Fprintln(w, "would remove "+path)
			return nil
		}
		return os.Remove(path)
	})
}

func shouldSkip(repo, path string) bool {
	base := filepath.Base(path)
	return base == ".git" ||
		path == filepath.Join(repo, ".ds") ||
		path == filepath.Join(repo, "dataset.yaml") ||
		path == filepath.Join(repo, ".gitignore") ||
		strings.Contains(path, string(filepath.Separator)+".ds"+string(filepath.Separator))
}

func absFromRepo(repo, p string) string {
	if filepath.IsAbs(p) {
		return filepath.Clean(p)
	}
	return filepath.Join(repo, p)
}

func rel(repo, p string) string {
	r, err := filepath.Rel(repo, p)
	if err != nil {
		return p
	}
	return r
}

func (a App) say(s string) {
	if !a.Quiet {
		fmt.Fprintln(a.Stdout, s)
	}
}
