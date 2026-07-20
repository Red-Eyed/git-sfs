package core

import (
	"context"
	"fmt"
	"path/filepath"

	"git-sfs/internal/cache"
	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
	"git-sfs/internal/lock"
)

type PushOptions struct {
	// SkipMissing uploads the files that are cached locally instead of failing
	// on the first one that is not. Off by default: a push that silently omits
	// files looks identical to a complete one, and a user who then trusts the
	// remote as a backup can lose the omitted data.
	SkipMissing bool
}

// Push uploads each referenced cache file to the remote in a single rclone call.
// Existing remote files are never overwritten.
//
// When a path is given, only symlinks below that path are uploaded. This mirrors
// Pull and is what makes a partially-pulled dataset pushable: subtrees the user
// deliberately never fetched are dangling symlinks, and scanning them would
// abort the push on a cache file that was never supposed to be local.
func (a App) Push(ctx context.Context, name, path string) error {
	return a.PushWithOptions(ctx, name, path, PushOptions{})
}

// PushWithOptions uploads referenced cache files below path to the remote.
func (a App) PushWithOptions(ctx context.Context, name, path string, opts PushOptions) (err error) {
	a.debugf("push: start remote=%s path=%s skip_missing=%t", name, path, opts.SkipMissing)
	defer a.debugDone("push", &err)
	s, err := a.open()
	if err != nil {
		return err
	}
	repo, c, cfg := s.repo, s.cache, s.cfg
	l, err := lock.Acquire(ctx, c.LocksDir(), "push")
	if err != nil {
		return err
	}
	defer l.Release()
	r, err := a.selectRemote(s, name)
	if err != nil {
		return err
	}
	if err := a.preflight(ctx, cfg, r); err != nil {
		return err
	}
	links, err := collectGitSFSSymlinks(repo, path)
	if err != nil {
		return err
	}
	present, missing := partitionByCachePresence(ctx, c, uniqueHashesFromTracked(links))
	if len(missing) > 0 && !opts.SkipMissing {
		return missingCachedFileError(repo, links, missing[0])
	}
	a.reportSkipped(repo, links, missing)
	relPaths := make([]string, 0, len(present))
	for _, h := range present {
		relPaths = append(relPaths, hash.Algorithm+"/"+h.Prefix()+"/"+h.String())
	}
	if len(relPaths) > 0 {
		a.say(fmt.Sprintf("push: uploading %d file(s) to remote", len(relPaths)))
	}
	return r.CopyToRemote(ctx, filepath.Join(c.Root, "files"), relPaths)
}

// partitionByCachePresence splits hashes into those backed by a valid local
// cache file and those that are absent or corrupt, preserving input order so
// the reported results are deterministic.
func partitionByCachePresence(ctx context.Context, c cache.Cache, hashes []hash.Hash) (present, missing []hash.Hash) {
	for _, h := range hashes {
		if c.HasValid(ctx, h) {
			present = append(present, h)
			continue
		}
		missing = append(missing, h)
	}
	return present, missing
}

// maxSkippedListed caps the per-path listing. A partially-pulled dataset can
// have thousands of dangling symlinks, and printing all of them would bury the
// upload result; git-sfs status prints the full list on demand.
const maxSkippedListed = 10

// reportSkipped names the paths left out of the upload. Skipping is opt-in, but
// it still makes the remote an incomplete copy, so the omission is written to
// stderr where it survives being piped and cannot be mistaken for success.
//
// Counts follow git-sfs status: unique cached files, then the symlinks that
// reference them. The two differ whenever paths share content, and reporting
// only the object count would understate how much of the tree is unbacked.
func (a App) reportSkipped(repo string, links []trackedLink, missing []hash.Hash) {
	if len(missing) == 0 {
		return
	}
	affected := linksReferencing(links, missing)
	fmt.Fprintf(a.Stderr,
		"git-sfs: warning: push skipped %d file(s) referenced by %d symlink(s); the remote is not a complete copy\n",
		len(missing), len(affected))
	for i, l := range affected {
		if i == maxSkippedListed {
			fmt.Fprintf(a.Stderr, "  ... and %d more (run: git-sfs status to list all)\n", len(affected)-i)
			break
		}
		fmt.Fprintf(a.Stderr, "  %s (%s)\n", rel(repo, l.Path), l.Hash.Short())
	}
	fmt.Fprintf(a.Stderr, "  run: git-sfs pull <path> to restore them\n")
}

// missingCachedFileError names a working-tree path that references h, so the
// user sees a file they recognize and the command that restores it rather than
// a bare hash. links is sorted by path, so the reported path is deterministic.
func missingCachedFileError(repo string, links []trackedLink, h hash.Hash) error {
	for _, l := range links {
		if l.Hash != h {
			continue
		}
		p := rel(repo, l.Path)
		return fmt.Errorf("%w: %s (%s): run: git-sfs pull %s", errs.ErrMissingCachedFile, p, h.Short(), p)
	}
	return fmt.Errorf("%w: %s", errs.ErrMissingCachedFile, h.Short())
}

// linksReferencing returns every link whose hash is in hashes, preserving the
// sorted-by-path order of links. Reporting has to expand objects back into paths
// because a single cached file can back any number of symlinks, and the user
// needs to know which of their files are affected, not which objects.
func linksReferencing(links []trackedLink, hashes []hash.Hash) []trackedLink {
	want := make(map[hash.Hash]bool, len(hashes))
	for _, h := range hashes {
		want[h] = true
	}
	var out []trackedLink
	for _, l := range links {
		if want[l.Hash] {
			out = append(out, l)
		}
	}
	return out
}
