package core

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/hash"
	"git-sfs/internal/localstate"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/remote"
	"git-sfs/internal/sfspath"
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
	DryRun bool
	Files  bool
}

type issue struct {
	Kind   string
	Path   string
	Hash   string
	Detail string
}

type statusReport struct {
	TrackedSymlinks int
	Issues          []issue
}

var issueKinds = []string{
	"unconverted file",
	"broken git symlink",
	"missing cache file",
	"corrupt cache file",
	"invalid config",
}

// Init creates the tracked project config and the untracked .git-sfs workspace.
func (a App) Init(ctx context.Context, force bool) error {
	repo, err := localstate.ResolveRepo()
	if err != nil {
		return err
	}
	cfgPath := filepath.Join(repo, a.ConfigPath)
	if _, err := os.Stat(cfgPath); err == nil && !force {
		return fmt.Errorf("%s already exists; use --force to overwrite", a.ConfigPath)
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
func (a App) Setup(ctx context.Context) error {
	repo, c, _, err := a.open()
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
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if c.HasValid(h) {
			if err := c.Protect(h); err != nil {
				return err
			}
			if err := materialize.Link(repo, c, h); err != nil {
				return err
			}
		}
	}
	a.say("setup complete")
	return nil
}

// Add converts regular files into git-sfs symlinks after copying their bytes into
// the local content-addressed cache.
func (a App) Add(ctx context.Context, paths []string) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
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
	for _, file := range files {
		h, err := hash.File(file)
		if err != nil {
			return err
		}
		if err := c.Store(file, h); err != nil {
			return fmt.Errorf("store %s: %w", file, err)
		}
		if err := c.Protect(h); err != nil {
			return err
		}
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

// Status reports user-actionable problems without mutating repository state.
func (a App) Status(ctx context.Context) error {
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	report, err := scan(repo, c)
	if err != nil {
		return err
	}
	if _, ok := cfg.Remotes["default"]; !ok {
		report.Issues = append(report.Issues, issue{
			Kind:   "invalid config",
			Detail: "missing default remote",
		})
	}
	printReport(a.Stdout, report)
	if len(report.Issues) > 0 {
		return fmt.Errorf("status found %d issue(s)", len(report.Issues))
	}
	return nil
}

// Verify is the CI-oriented strict check; any reported problem is a failure.
func (a App) Verify(ctx context.Context) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	report, err := scan(repo, c)
	if err != nil {
		return err
	}
	if len(report.Issues) > 0 {
		printReport(a.Stdout, report)
		return fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
	}
	a.say("verify ok")
	return nil
}

func (a App) Materialize(ctx context.Context, path string) error {
	repo, c, _, err := a.open()
	if err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if err := c.Protect(h); err != nil {
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
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
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
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	seen := map[hash.Hash]bool{}
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
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

// Pull downloads missing files for the selected symlinks.
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
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		if !c.HasValid(h) {
			dst := c.FilePath(h)
			if err := r.PullFile(ctx, h, dst); err != nil {
				return fmt.Errorf("pull %s: %w", h, err)
			}
		}
		if err := c.Protect(h); err != nil {
			return err
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
	if !opts.Files {
		opts.Files = true
	}
	links, err := collectGitSFSSymlinks(repo, ".")
	if err != nil {
		return err
	}
	live := map[string]bool{}
	for _, l := range links {
		h, _, err := sfspath.ParseGitSymlink(repo, l)
		if err != nil {
			return err
		}
		live[h.String()] = true
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
	if err := localstate.BindCache(repo, c); err != nil {
		return "", cache.Cache{}, config.Config{}, err
	}
	return repo, c, cfg, nil
}

func scan(repo string, c cache.Cache) (statusReport, error) {
	var report statusReport
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
				report.Issues = append(report.Issues, issue{
					Kind: "unconverted file",
					Path: rel(repo, path),
				})
			}
			return nil
		}
		h, _, err := sfspath.ParseGitSymlink(repo, path)
		if err != nil {
			report.Issues = append(report.Issues, issue{
				Kind:   "broken git symlink",
				Path:   rel(repo, path),
				Detail: err.Error(),
			})
			return nil
		}
		report.TrackedSymlinks++
		cacheFile := c.FilePath(h)
		if _, err := os.Stat(cacheFile); err != nil {
			report.Issues = append(report.Issues, issue{
				Kind: "missing cache file",
				Path: rel(repo, path),
				Hash: h.String(),
			})
			return nil
		}
		if err := hash.VerifyFile(cacheFile, h); err != nil {
			report.Issues = append(report.Issues, issue{
				Kind:   "corrupt cache file",
				Path:   rel(repo, path),
				Hash:   h.String(),
				Detail: err.Error(),
			})
			return nil
		}
		return nil
	})
	return report, err
}

func printReport(w io.Writer, report statusReport) {
	counts := map[string]int{}
	for _, item := range report.Issues {
		counts[item.Kind]++
	}
	fmt.Fprintf(w, "tracked symlinks: %d\n", report.TrackedSymlinks)
	for _, kind := range issueKinds {
		fmt.Fprintf(w, "%s: %d\n", pluralKind(kind), counts[kind])
	}
	if len(report.Issues) == 0 {
		return
	}
	fmt.Fprintln(w, "details:")
	for _, item := range report.Issues {
		fmt.Fprintln(w, formatIssue(item))
	}
}

func formatIssue(item issue) string {
	parts := []string{item.Kind}
	if item.Path != "" {
		parts = append(parts, item.Path)
	}
	if item.Hash != "" {
		parts = append(parts, item.Hash)
	}
	out := strings.Join(parts, ": ")
	if item.Detail != "" {
		out += ": " + item.Detail
	}
	return out
}

func pluralKind(kind string) string {
	switch kind {
	case "unconverted file":
		return "unconverted files"
	case "broken git symlink":
		return "broken git symlinks"
	case "missing cache file":
		return "missing cache files"
	case "corrupt cache file":
		return "corrupt cache files"
	case "invalid config":
		return "invalid config"
	default:
		return kind
	}
}

func collectGitSFSSymlinks(repo, path string) ([]string, error) {
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
		if info.Mode()&os.ModeSymlink != 0 && sfspath.IsSFSSymlink(repo, path) {
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
		path == filepath.Join(repo, ".git-sfs") ||
		path == filepath.Join(repo, ".git-sfs/config.toml") ||
		path == filepath.Join(repo, ".gitignore") ||
		strings.Contains(path, string(filepath.Separator)+".git-sfs"+string(filepath.Separator))
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
