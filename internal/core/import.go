package core

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"git-sfs/internal/cache"
	"git-sfs/internal/hash"
	"git-sfs/internal/localstate"
	"git-sfs/internal/lock"
	"git-sfs/internal/materialize"
	"git-sfs/internal/progress"
	"git-sfs/internal/sfspath"
)

type ImportOptions struct {
	FollowSymlinks bool
	Move           bool // delete source after caching; default is copy (leave source intact)
}

type movePair struct {
	Src string
	Dst string
	Key string
}

type importPrepared struct {
	Key  string
	Hash hash.Hash
	Err  error
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
	prepared := prepareImportFiles(ctx, c, pairs, opts, a.jobs(cfg, len(pairs)))
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
	if opts.Move {
		removeSourceLinks(links)
		removeEmptyDirs(dirs)
	}
	return nil
}

func prepareImportFiles(ctx context.Context, c cache.Cache, pairs []movePair, opts ImportOptions, workers int) []importPrepared {
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
		var cacheErr error
		if opts.Move {
			cacheErr = c.Move(pair.Src, h)
		} else {
			cacheErr = c.Store(pair.Src, h)
		}
		if cacheErr != nil {
			return fmt.Errorf("import %s: %w", pair.Src, cacheErr)
		}
		out[i] = importPrepared{Key: pair.Key, Hash: h}
		return nil
	}, func(i int, err error) {
		out[i] = importPrepared{Key: unique[i].Key, Err: err}
	})
	return out
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
