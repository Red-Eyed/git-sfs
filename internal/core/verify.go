package core

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"git-sfs/internal/cache"
	"git-sfs/internal/config"
	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
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
	repo, c, cfg, err := a.open()
	if err != nil {
		return err
	}

	var r remote.Remote
	if checkRemote {
		if _, ok := cfg.Remotes[nameOrDefault(remoteName)]; !ok {
			report := statusReport{Issues: []issue{{Kind: "invalid config", Detail: "missing default remote"}}}
			printReport(a.Stdout, report)
			return fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
		}
		r, err = a.selectRemote(repo, cfg, remoteName)
		if err != nil {
			report := statusReport{Issues: []issue{{Kind: "invalid config", Detail: err.Error()}}}
			printReport(a.Stdout, report)
			return fmt.Errorf("verify failed with %d issue(s)", len(report.Issues))
		}
	}

	report, err := scan(ctx, repo, path, c, cfg, r, checkRemote, withIntegrity)
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

func nameOrDefault(name string) string {
	if name == "" {
		return "default"
	}
	return name
}

func scan(ctx context.Context, repo, path string, c cache.Cache, cfg config.Config, r remote.Remote, checkRemote, withIntegrity bool) (statusReport, error) {
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
	remStatus, err := checkRemoteFiles(ctx, r, tracked, withIntegrity, jobsFromSettings(cfg.Settings.Jobs, len(tracked)))
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

func checkCacheFiles(ctx context.Context, c cache.Cache, tracked []trackedLink, withIntegrity bool, workers int) map[hash.Hash]remoteStatus {
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
		if withIntegrity {
			if err := hash.VerifyFile(cacheFile, h); err != nil {
				return remoteStatus{Err: err}
			}
			if info.Mode().Perm()&0o222 != 0 {
				return remoteStatus{Err: fmt.Errorf("cache file is writable: %w", errs.ErrWrongCachePermissions)}
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
	hashes := uniqueHashesFromTracked(tracked)
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
