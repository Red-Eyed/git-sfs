package core

import (
	"os"
	"path/filepath"
	"sort"
	"strings"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

type trackedLink struct {
	Path string
	Hash hash.Hash
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
