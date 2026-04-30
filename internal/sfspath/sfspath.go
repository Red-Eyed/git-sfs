package sfspath

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"git-sfs/internal/errs"
	"git-sfs/internal/hash"
)

// CacheLinkFile is the repo-local path exposed through .git-sfs/cache.
func CacheLinkFile(repo string, h hash.Hash) string {
	return filepath.Join(repo, ".git-sfs", "cache", "files", hash.Algorithm, h.Prefix(), h.String())
}

// GitLinkTarget returns the relative symlink target that is safe to commit.
func GitLinkTarget(repo, file string, h hash.Hash) (string, error) {
	return filepath.Rel(filepath.Dir(file), CacheLinkFile(repo, h))
}

// ParseGitSymlink validates the committed symlink format and extracts its hash.
func ParseGitSymlink(repo, file string) (hash.Hash, string, error) {
	target, err := os.Readlink(file)
	if err != nil {
		return "", "", err
	}
	if filepath.IsAbs(target) {
		return "", target, errors.Join(errs.ErrInvalidSymlink, fmt.Errorf("git symlink %s has absolute target %s", file, target))
	}
	// Git symlinks point through .git-sfs/cache so machine-local cache paths
	// never appear in committed metadata.
	resolved := filepath.Clean(filepath.Join(filepath.Dir(file), target))
	wantRoot := filepath.Join(repo, ".git-sfs", "cache", "files", hash.Algorithm)
	rel, err := filepath.Rel(wantRoot, resolved)
	if err != nil || strings.HasPrefix(rel, "..") || rel == "." {
		return "", target, errors.Join(errs.ErrInvalidSymlink, fmt.Errorf("git symlink %s does not point into .git-sfs/cache", file))
	}
	parts := strings.Split(filepath.ToSlash(rel), "/")
	if len(parts) != 2 {
		return "", target, errors.Join(errs.ErrInvalidSymlink, fmt.Errorf("git symlink %s has invalid file path", file))
	}
	// The first path component is redundant with the hash, but checking it keeps
	// stale or hand-edited links from silently pointing at the wrong layout.
	h, err := hash.Parse(parts[1])
	if err != nil {
		return "", target, err
	}
	if parts[0] != h.Prefix() {
		return "", target, errors.Join(errs.ErrInvalidSymlink, fmt.Errorf("git symlink %s prefix %q does not match hash", file, parts[0]))
	}
	return h, target, nil
}

func IsSFSSymlink(repo, file string) bool {
	info, err := os.Lstat(file)
	if err != nil || info.Mode()&os.ModeSymlink == 0 {
		return false
	}
	_, _, err = ParseGitSymlink(repo, file)
	return err == nil
}
