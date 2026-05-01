package core

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/localstate"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/progress"
	"git-sfs/internal/remote"
	"git-sfs/internal/sfspath"
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

type ImportOptions struct {
	FollowSymlinks bool
}

type movePair struct {
	Src string
	Dst string
	Key string
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

type addPrepared struct {
	Hash hash.Hash
	Err  error
}

type importPrepared struct {
	Key  string
	Hash hash.Hash
	Err  error
}

type trackedLink struct {
	Path string
	Hash hash.Hash
}

type remoteStatus struct {
	OK  bool
	Err error
}

var issueKinds = []string{
	"unconverted file",
	"broken git symlink",
	"missing cache file",
	"corrupt cache file",
	"missing remote file",
	"corrupt remote file",
	"invalid config",
}

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

// Add converts regular files into git-sfs symlinks after copying their bytes into
// the local content-addressed cache.
func (a App) Add(ctx context.Context, paths []string) (err error) {
	a.debugf("add: start paths=%d", len(paths))
	defer a.debugDone("add", &err)
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
	bar := progress.New(a.Stderr, "add", len(files), a.Quiet)
	defer bar.Close()
	prepared := prepareAddFiles(ctx, c, files, a.jobs(cfg, len(files)))
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
		bar.Step()
	}
	return nil
}

// Import ingests external files into the cache with renames and creates Git
// symlinks at the destination paths.
func (a App) Import(ctx context.Context, srcPath, dstPath string) error {
	return a.ImportWithOptions(ctx, srcPath, dstPath, ImportOptions{})
}

// ImportWithOptions ingests external files into the cache with renames and
// creates Git symlinks at the destination paths.
func (a App) ImportWithOptions(ctx context.Context, srcPath, dstPath string, opts ImportOptions) (err error) {
	a.debugf("import: start src=%s dst=%s follow_symlinks=%t", srcPath, dstPath, opts.FollowSymlinks)
	defer a.debugDone("import", &err)
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
	l, err := lock.Acquire(ctx, c.LocksDir(), "import")
	if err != nil {
		return err
	}
	defer l.Release()
	pairs, dirs, links, err := planMove(repo, srcPath, dstPath, opts)
	if err != nil {
		return err
	}
	bar := progress.New(a.Stderr, "import", len(pairs), a.Quiet)
	defer bar.Close()
	prepared := prepareImportFiles(ctx, c, pairs, a.jobs(cfg, len(pairs)))
	imported := map[string]hash.Hash{}
	for _, item := range prepared {
		if item.Err != nil {
			return item.Err
		}
		imported[item.Key] = item.Hash
	}
	for _, pair := range pairs {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		h, ok := imported[pair.Key]
		if !ok {
			return fmt.Errorf("missing prepared import for %s", pair.Src)
		}
		if err := materialize.Link(repo, c, h); err != nil {
			return err
		}
		target, err := sfspath.GitLinkTarget(repo, pair.Dst, h)
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(pair.Dst), 0o755); err != nil {
			return err
		}
		if err := os.Symlink(target, pair.Dst); err != nil {
			return err
		}
		a.say("imported " + pair.Src + " -> " + rel(repo, pair.Dst) + " -> " + h.String())
		bar.Step()
	}
	removeSourceLinks(links)
	removeEmptyDirs(dirs)
	return nil
}

// Verify is the CI-oriented strict check; any reported problem is a failure.
func (a App) Verify(ctx context.Context, checkRemote, withIntegrity bool, path string) (err error) {
	a.debugf("verify: start")
	defer a.debugDone("verify", &err)
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}
	report, err := scan(ctx, repo, path, c, cfg, checkRemote, withIntegrity)
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

func uniqueHashesFromTracked(links []trackedLink) []hash.Hash {
	seen := map[hash.Hash]bool{}
	hashes := make([]hash.Hash, 0, len(links))
	for _, l := range links {
		if seen[l.Hash] {
			continue
		}
		seen[l.Hash] = true
		hashes = append(hashes, l.Hash)
	}
	return hashes
}

// Pull downloads missing files for the selected symlinks.
func (a App) Pull(ctx context.Context, path string) (err error) {
	a.debugf("pull: start path=%s", path)
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
	r, err := a.selectRemote(repo, cfg, "")
	if err != nil {
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

func scan(ctx context.Context, repo, path string, c cache.Cache, cfg config.Config, checkRemote, withIntegrity bool) (statusReport, error) {
	var report statusReport
	defaultRemote, hasDefault := cfg.Remotes["default"]
	root := absFromRepo(repo, path)
	var tracked []trackedLink
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
		if d.Type()&os.ModeSymlink == 0 {
			if d.Type().IsRegular() {
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
		tracked = append(tracked, trackedLink{Path: rel(repo, path), Hash: h})
		return nil
	})
	if err != nil {
		return report, err
	}
	cacheStatus := checkCacheFiles(ctx, c, tracked, withIntegrity, jobsFromSettings(cfg.Settings.Jobs, len(tracked)))
	for _, item := range tracked {
		status := cacheStatus[item.Hash]
		switch {
		case errors.Is(status.Err, os.ErrNotExist):
			report.Issues = append(report.Issues, issue{
				Kind: "missing cache file",
				Path: item.Path,
				Hash: item.Hash.String(),
			})
		case status.Err != nil:
			report.Issues = append(report.Issues, issue{
				Kind:   "corrupt cache file",
				Path:   item.Path,
				Hash:   item.Hash.String(),
				Detail: status.Err.Error(),
			})
		}
	}
	if !checkRemote {
		return report, nil
	}
	if !hasDefault {
		report.Issues = append(report.Issues, issue{
			Kind:   "invalid config",
			Detail: "missing default remote",
		})
		return report, nil
	}
	r, err := remote.NewWithOptions(defaultRemote, remote.Options{})
	if err != nil {
		report.Issues = append(report.Issues, issue{
			Kind:   "invalid config",
			Detail: err.Error(),
		})
		return report, nil
	}
	remoteStatus, err := checkRemoteFiles(ctx, r, tracked, withIntegrity, jobsFromSettings(cfg.Settings.Jobs, len(tracked)))
	if err != nil {
		return report, err
	}
	for _, item := range tracked {
		status := remoteStatus[item.Hash]
		switch {
		case withIntegrity && errors.Is(status.Err, errs.ErrCorruptRemoteFile):
			report.Issues = append(report.Issues, issue{
				Kind:   "corrupt remote file",
				Path:   item.Path,
				Hash:   item.Hash.String(),
				Detail: status.Err.Error(),
			})
		case status.Err != nil:
			return report, status.Err
		case !status.OK:
			report.Issues = append(report.Issues, issue{
				Kind: "missing remote file",
				Path: item.Path,
				Hash: item.Hash.String(),
			})
		}
	}
	return report, nil
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
	case "missing remote file":
		return "missing remote files"
	case "corrupt remote file":
		return "corrupt remote files"
	case "invalid config":
		return "invalid config"
	default:
		return kind
	}
}

func collectGitSFSSymlinks(repo, path string) ([]trackedLink, error) {
	root := absFromRepo(repo, path)
	var out []trackedLink
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
		if d.Type()&os.ModeSymlink == 0 {
			return nil
		}
		h, _, err := sfspath.ParseGitSymlink(repo, path)
		if err != nil {
			return nil
		}
		out = append(out, trackedLink{Path: path, Hash: h})
		return nil
	})
	sort.Slice(out, func(i, j int) bool { return out[i].Path < out[j].Path })
	return out, err
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

func (a App) jobs(cfg config.Config, n int) int {
	if a.Jobs > 0 {
		return jobsFromSettings(a.Jobs, n)
	}
	return jobsFromSettings(cfg.Settings.Jobs, n)
}

func prepareAddFiles(ctx context.Context, c cache.Cache, files []string, workers int) []addPrepared {
	out := make([]addPrepared, len(files))
	runIndexed(ctx, len(files), workers, func(i int) error {
		h, err := hash.File(files[i])
		if err != nil {
			return err
		}
		if err := c.Store(files[i], h); err != nil {
			return fmt.Errorf("store %s: %w", files[i], err)
		}
		out[i].Hash = h
		return nil
	}, func(i int, err error) {
		out[i].Err = err
	})
	return out
}

func prepareImportFiles(ctx context.Context, c cache.Cache, pairs []movePair, workers int) []importPrepared {
	seen := map[string]movePair{}
	var unique []movePair
	for _, pair := range pairs {
		if _, ok := seen[pair.Key]; ok {
			continue
		}
		seen[pair.Key] = pair
		unique = append(unique, pair)
	}
	out := make([]importPrepared, len(unique))
	runIndexed(ctx, len(unique), workers, func(i int) error {
		pair := unique[i]
		h, err := hash.File(pair.Src)
		if err != nil {
			return err
		}
		if err := c.Move(pair.Src, h); err != nil {
			return fmt.Errorf("import %s: %w", pair.Src, err)
		}
		out[i] = importPrepared{Key: pair.Key, Hash: h}
		return nil
	}, func(i int, err error) {
		out[i] = importPrepared{Key: unique[i].Key, Err: err}
	})
	return out
}

func pullMissingFiles(ctx context.Context, c cache.Cache, r remote.Remote, hashes []hash.Hash, workers int) error {
	errsByIndex := make([]error, len(hashes))
	runIndexed(ctx, len(hashes), workers, func(i int) error {
		h := hashes[i]
		if c.HasValid(h) {
			return nil
		}
		if err := r.PullFile(ctx, h, c.FilePath(h)); err != nil {
			return fmt.Errorf("pull %s: %w", h, err)
		}
		return nil
	}, func(i int, err error) {
		errsByIndex[i] = err
	})
	for _, err := range errsByIndex {
		if err != nil {
			return err
		}
	}
	return nil
}

func checkCacheFiles(ctx context.Context, c cache.Cache, tracked []trackedLink, withIntegrity bool, workers int) map[hash.Hash]remoteStatus {
	hashes := uniqueTrackedHashes(tracked)
	out := make(map[hash.Hash]remoteStatus, len(hashes))
	var mu sync.Mutex
	runHashes(ctx, hashes, workers, func(h hash.Hash) remoteStatus {
		select {
		case <-ctx.Done():
			return remoteStatus{Err: ctx.Err()}
		default:
		}
		cacheFile := c.FilePath(h)
		if _, err := os.Stat(cacheFile); err != nil {
			return remoteStatus{Err: err}
		}
		if withIntegrity {
			if err := hash.VerifyFile(cacheFile, h); err != nil {
				return remoteStatus{Err: err}
			}
		}
		return remoteStatus{OK: true}
	}, func(error) bool {
		return false
	}, func(h hash.Hash, status remoteStatus) {
		mu.Lock()
		out[h] = status
		mu.Unlock()
	})
	return out
}

func checkRemoteFiles(ctx context.Context, r remote.Remote, tracked []trackedLink, withIntegrity bool, workers int) (map[hash.Hash]remoteStatus, error) {
	hashes := uniqueTrackedHashes(tracked)
	out := make(map[hash.Hash]remoteStatus, len(hashes))
	var mu sync.Mutex
	var firstErr error
	var once sync.Once
	runHashes(ctx, hashes, workers, func(h hash.Hash) remoteStatus {
		var (
			ok  bool
			err error
		)
		if withIntegrity {
			ok, err = r.CheckFile(ctx, h)
		} else {
			ok, err = r.HasFile(ctx, h)
		}
		return remoteStatus{OK: ok, Err: err}
	}, func(err error) bool {
		return !(withIntegrity && errors.Is(err, errs.ErrCorruptRemoteFile))
	}, func(h hash.Hash, status remoteStatus) {
		mu.Lock()
		out[h] = status
		mu.Unlock()
		if status.Err != nil && !(withIntegrity && errors.Is(status.Err, errs.ErrCorruptRemoteFile)) {
			once.Do(func() {
				firstErr = status.Err
			})
		}
	})
	return out, firstErr
}

func uniqueTrackedHashes(tracked []trackedLink) []hash.Hash {
	seen := map[hash.Hash]bool{}
	hashes := make([]hash.Hash, 0, len(tracked))
	for _, item := range tracked {
		if seen[item.Hash] {
			continue
		}
		seen[item.Hash] = true
		hashes = append(hashes, item.Hash)
	}
	return hashes
}

func runIndexed(ctx context.Context, count, workers int, work func(int) error, fail func(int, error)) {
	if count == 0 {
		return
	}
	workers = jobsFromSettings(workers, count)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	jobs := make(chan int)
	var once sync.Once
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for idx := range jobs {
				if ctx.Err() != nil {
					return
				}
				if err := work(idx); err != nil {
					fail(idx, err)
					once.Do(cancel)
					return
				}
			}
		}()
	}
	for i := 0; i < count; i++ {
		select {
		case <-ctx.Done():
			close(jobs)
			wg.Wait()
			return
		case jobs <- i:
		}
	}
	close(jobs)
	wg.Wait()
}

func runHashes(ctx context.Context, hashes []hash.Hash, workers int, work func(hash.Hash) remoteStatus, stopOn func(error) bool, store func(hash.Hash, remoteStatus)) {
	runIndexed(ctx, len(hashes), workers, func(i int) error {
		status := work(hashes[i])
		store(hashes[i], status)
		if status.Err != nil && stopOn(status.Err) {
			return status.Err
		}
		return nil
	}, func(i int, err error) {})
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


func shouldSkip(repo, path string) bool {
	base := filepath.Base(path)
	return base == ".git" ||
		path == filepath.Join(repo, ".git-sfs") ||
		path == filepath.Join(repo, ".git-sfs/config.toml") ||
		path == filepath.Join(repo, ".gitignore") ||
		strings.Contains(path, string(filepath.Separator)+".git-sfs"+string(filepath.Separator))
}

func planMove(repo, srcPath, dstPath string, opts ImportOptions) ([]movePair, []string, []string, error) {
	src, err := filepath.Abs(srcPath)
	if err != nil {
		return nil, nil, nil, err
	}
	sourceLink := src
	dst := absFromRepo(repo, dstPath)
	relDst, err := filepath.Rel(repo, dst)
	if err != nil || relDst == "." || strings.HasPrefix(relDst, "..") {
		return nil, nil, nil, fmt.Errorf("destination must be inside repository: %s", dstPath)
	}
	if shouldSkip(repo, dst) || strings.Contains(dst, string(filepath.Separator)+".git-sfs"+string(filepath.Separator)) {
		return nil, nil, nil, fmt.Errorf("destination must not be inside .git-sfs: %s", dstPath)
	}
	info, err := os.Lstat(src)
	if err != nil {
		return nil, nil, nil, err
	}
	var links []string
	if info.Mode()&os.ModeSymlink != 0 {
		if !opts.FollowSymlinks {
			return nil, nil, nil, fmt.Errorf("source symlinks are not supported without -L: %s", srcPath)
		}
		resolved, err := filepath.EvalSymlinks(src)
		if err != nil {
			return nil, nil, nil, fmt.Errorf("resolve source symlink %s: %w", srcPath, err)
		}
		info, err = os.Lstat(resolved)
		if err != nil {
			return nil, nil, nil, err
		}
		src = resolved
		links = append(links, sourceLink)
	}
	if info.Mode().IsRegular() {
		if _, err := os.Lstat(dst); err == nil {
			return nil, nil, nil, fmt.Errorf("destination already exists: %s", dstPath)
		} else if !os.IsNotExist(err) {
			return nil, nil, nil, err
		}
		return []movePair{{Src: src, Dst: dst, Key: canonicalSourceKey(src)}}, nil, links, nil
	}
	if !info.IsDir() {
		return nil, nil, nil, fmt.Errorf("source must be a regular file or directory: %s", srcPath)
	}
	if st, err := os.Lstat(dst); err == nil && !st.IsDir() {
		return nil, nil, nil, fmt.Errorf("destination exists and is not a directory: %s", dstPath)
	} else if err != nil && !os.IsNotExist(err) {
		return nil, nil, nil, err
	}
	var pairs []movePair
	var dirs []string
	err = filepath.WalkDir(src, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if path == src {
			return nil
		}
		relPath, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}
		if d.IsDir() {
			dirs = append(dirs, path)
			return nil
		}
		srcFile := path
		if d.Type()&os.ModeSymlink != 0 {
			if !opts.FollowSymlinks {
				return fmt.Errorf("source contains symlink without -L: %s", path)
			}
			resolved, err := filepath.EvalSymlinks(path)
			if err != nil {
				return fmt.Errorf("resolve source symlink %s: %w", path, err)
			}
			info, err := os.Lstat(resolved)
			if err != nil {
				return err
			}
			if !info.Mode().IsRegular() {
				return fmt.Errorf("source symlink must resolve to a regular file: %s", path)
			}
			srcFile = resolved
			links = append(links, path)
		} else if !d.Type().IsRegular() {
			return fmt.Errorf("source contains unsupported non-regular file: %s", path)
		}
		out := filepath.Join(dst, relPath)
		if _, err := os.Lstat(out); err == nil {
			return fmt.Errorf("destination already exists: %s", out)
		} else if !os.IsNotExist(err) {
			return err
		}
		pairs = append(pairs, movePair{Src: srcFile, Dst: out, Key: canonicalSourceKey(srcFile)})
		return nil
	})
	if err != nil {
		return nil, nil, nil, err
	}
	sort.Slice(dirs, func(i, j int) bool { return len(dirs[i]) > len(dirs[j]) })
	dirs = append(dirs, src)
	return pairs, dirs, links, nil
}

func removeEmptyDirs(dirs []string) {
	for _, dir := range dirs {
		_ = os.Remove(dir)
	}
}

func removeSourceLinks(links []string) {
	for _, link := range links {
		_ = os.Remove(link)
	}
}

func canonicalSourceKey(path string) string {
	resolved, err := filepath.EvalSymlinks(path)
	if err == nil {
		return resolved
	}
	return path
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

func (a App) debugf(format string, args ...any) {
	if !a.Verbose {
		return
	}
	fmt.Fprintf(a.Stderr, "debug: "+format+"\n", args...)
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
