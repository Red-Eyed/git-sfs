package core

import (
	"context"
	"errors"
	"fmt"
	"io"
	"math/rand"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/progress"
	"git-sfs/internal/remote"
	"git-sfs/internal/sfspath"
)

type issue struct {
	Kind   string
	Path   string
	Hash   string
	Detail string
}

type statusReport struct {
	TrackedSymlinks int
	OrphanCount     int
	TrackedHashes   map[string]struct{}
	Issues          []issue
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
	"wrong cache permissions",
	"missing remote file",
	"corrupt remote file",
	"invalid config",
}

// Verify is the CI-oriented strict check; any reported problem is a failure.
func (a App) Verify(ctx context.Context, remoteName string, checkRemote, withIntegrity bool, path string) (err error) {
	a.debugf("verify: start")
	defer a.debugDone("verify", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	repo, c, cfg := s.repo, s.cache, s.cfg

	var r remote.Remote
	if checkRemote {
		if _, ok := cfg.Remotes[nameOrDefault(remoteName)]; !ok {
			report := statusReport{Issues: []issue{{Kind: "invalid config", Detail: "missing default remote"}}}
			printReport(a.Stdout, report)
			return fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
		}
		r, err = a.selectRemote(s, remoteName)
		if err != nil {
			report := statusReport{Issues: []issue{{Kind: "invalid config", Detail: err.Error()}}}
			printReport(a.Stdout, report)
			return fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
		}
		if err := a.preflight(ctx, cfg, r); err != nil {
			return err
		}
	}

	report, err := scan(ctx, repo, path, c, cfg, r, checkRemote, withIntegrity, a.Stderr, a.Quiet)
	if err != nil {
		return err
	}
	orphans, err := countOrphans(c, report)
	if err != nil {
		return err
	}
	report.OrphanCount = orphans
	printReport(a.Stdout, report)
	if len(report.Issues) > 0 {
		return verifyError(report)
	}
	a.say("verify ok")
	return nil
}

// verifyError returns a typed error whose sentinel reflects the most severe issue kind,
// so the process exits with the right code (3 for integrity failures, 2 otherwise).
func verifyError(report statusReport) error {
	base := fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
	for _, item := range report.Issues {
		switch item.Kind {
		case "corrupt cache file", "wrong cache permissions":
			return fmt.Errorf("%w: %w", errs.ErrCorruptCachedFile, base)
		case "corrupt remote file":
			return fmt.Errorf("%w: %w", errs.ErrCorruptRemoteFile, base)
		}
	}
	return base
}

func nameOrDefault(name string) string {
	if name == "" {
		return "default"
	}
	return name
}

func scan(ctx context.Context, repo, path string, c cache.Cache, cfg config.Config, r remote.Remote, checkRemote, withIntegrity bool, stderr io.Writer, quiet bool) (statusReport, error) {
	var report statusReport
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
	report.TrackedHashes = make(map[string]struct{}, len(tracked))
	for _, item := range tracked {
		report.TrackedHashes[item.Hash.String()] = struct{}{}
	}

	workers := jobsFromSettings(cfg.Settings.Jobs, len(tracked))
	hashes := uniqueHashesFromTracked(tracked)

	localBar := progress.New(stderr, "verify local", len(hashes), quiet)
	cacheStatus := checkCacheFiles(ctx, c, tracked, withIntegrity, workers, localBar.Step)
	localBar.Close()

	for _, item := range tracked {
		status := cacheStatus[item.Hash]
		switch {
		case errors.Is(status.Err, os.ErrNotExist):
			report.Issues = append(report.Issues, issue{
				Kind: "missing cache file",
				Path: item.Path,
				Hash: item.Hash.String(),
			})
		case errors.Is(status.Err, errs.ErrWrongCachePermissions):
			report.Issues = append(report.Issues, issue{
				Kind:   "wrong cache permissions",
				Path:   item.Path,
				Hash:   item.Hash.String(),
				Detail: status.Err.Error(),
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

	remoteBar := progress.New(stderr, "verify remote", len(hashes), quiet)
	remStatus, err := checkRemoteFiles(ctx, r, tracked, withIntegrity, workers, remoteBar.Step)
	remoteBar.Close()
	if err != nil {
		return report, err
	}
	for _, item := range tracked {
		status := remStatus[item.Hash]
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

func checkCacheFiles(ctx context.Context, c cache.Cache, tracked []trackedLink, withIntegrity bool, workers int, onStep func()) map[hash.Hash]remoteStatus {
	hashes := uniqueHashesFromTracked(tracked)
	out := make(map[hash.Hash]remoteStatus, len(hashes))
	var mu sync.Mutex
	runHashes(ctx, hashes, workers, func(h hash.Hash) remoteStatus {
		select {
		case <-ctx.Done():
			return remoteStatus{Err: ctx.Err()}
		default:
		}
		cacheFile := c.FilePath(h)
		info, err := os.Stat(cacheFile)
		if err != nil {
			return remoteStatus{Err: err}
		}
		if info.Mode().Perm()&0o222 != 0 {
			return remoteStatus{Err: fmt.Errorf("cache file is writable: %w", errs.ErrWrongCachePermissions)}
		}
		if withIntegrity {
			if err := hash.VerifyFile(ctx, cacheFile, h); err != nil {
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
		onStep()
	})
	return out
}

func checkRemoteFiles(ctx context.Context, r remote.Remote, tracked []trackedLink, withIntegrity bool, workers int, onStep func()) (map[hash.Hash]remoteStatus, error) {
	hashes := uniqueHashesFromTracked(tracked)
	out := make(map[hash.Hash]remoteStatus, len(hashes))

	if !withIntegrity {
		// One batch listing instead of one rclone process per file.
		sizes, err := r.FileSizes(ctx, hashes)
		if err != nil {
			return out, err
		}
		for _, h := range hashes {
			_, found := sizes[h]
			out[h] = remoteStatus{OK: found}
			onStep()
		}
		return out, nil
	}

	// Integrity check must download and hash-verify each file individually.
	var mu sync.Mutex
	var firstErr error
	var once sync.Once
	runHashes(ctx, hashes, workers, func(h hash.Hash) remoteStatus {
		ok, err := r.CheckFile(ctx, h)
		return remoteStatus{OK: ok, Err: err}
	}, func(err error) bool {
		return !errors.Is(err, errs.ErrCorruptRemoteFile)
	}, func(h hash.Hash, status remoteStatus) {
		mu.Lock()
		out[h] = status
		mu.Unlock()
		onStep()
		if status.Err != nil && !errors.Is(status.Err, errs.ErrCorruptRemoteFile) {
			once.Do(func() { firstErr = status.Err })
		}
	})
	return out, firstErr
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

// countOrphans counts cache files that are not referenced by any tracked symlink.
func countOrphans(c cache.Cache, report statusReport) (int, error) {
	root := filepath.Join(c.Root, "files", hash.Algorithm)
	if _, err := os.Stat(root); os.IsNotExist(err) {
		return 0, nil
	}
	orphans := 0
	err := filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		if _, ok := report.TrackedHashes[d.Name()]; !ok {
			orphans++
		}
		return nil
	})
	return orphans, err
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
	if report.OrphanCount > 0 {
		fmt.Fprintf(w, "# %d orphaned cache object(s) (run git-sfs gc to reclaim)\n", report.OrphanCount)
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

// RehashCache re-hashes every file in the local cache to detect silent
// corruption (bit rot). When sample > 0, only that many randomly chosen files
// are checked — useful for periodic spot-checks at terabyte scale.
func (a App) RehashCache(ctx context.Context, sample int) (err error) {
	a.debugf("rehash: start sample=%d", sample)
	defer a.debugDone("rehash", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	c := s.cache
	root := filepath.Join(c.Root, "files", hash.Algorithm)
	if _, err := os.Stat(root); os.IsNotExist(err) {
		a.say("rehash: cache is empty")
		return nil
	}

	var paths []string
	if err := filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		paths = append(paths, p)
		return nil
	}); err != nil {
		return fmt.Errorf("rehash: walk cache: %w", err)
	}

	if sample > 0 && sample < len(paths) {
		rand.Shuffle(len(paths), func(i, j int) { paths[i], paths[j] = paths[j], paths[i] })
		paths = paths[:sample]
	}

	if len(paths) == 0 {
		a.say("rehash: no cache files found")
		return nil
	}

	cfg := s.cfg
	workers := jobsFromSettings(cfg.Settings.Jobs, len(paths))
	bar := progress.New(a.Stderr, "rehash", len(paths), a.Quiet)

	var mu sync.Mutex
	var mismatches []string
	runIndexed(ctx, len(paths), workers, func(i int) error {
		p := paths[i]
		name := filepath.Base(p)
		h, parseErr := hash.Parse(name)
		if parseErr != nil {
			// Not a hash-named file; skip silently.
			bar.Step()
			return nil
		}
		if verifyErr := hash.VerifyFile(ctx, p, h); verifyErr != nil {
			mu.Lock()
			mismatches = append(mismatches, p+": "+verifyErr.Error())
			mu.Unlock()
		}
		bar.Step()
		return nil
	}, func(_ int, _ error) {})
	bar.Close()

	for _, msg := range mismatches {
		fmt.Fprintln(a.Stdout, "CORRUPT: "+msg)
	}
	if len(mismatches) > 0 {
		return fmt.Errorf("rehash: %d corrupt cache file(s) detected", len(mismatches))
	}
	a.say(fmt.Sprintf("rehash ok: %d file(s) verified", len(paths)))
	return nil
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
	case "wrong cache permissions":
		return "wrong cache permissions"
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
