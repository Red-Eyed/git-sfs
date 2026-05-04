package core

import (
	"fmt"
	"os"
	"path/filepath"

	"git-sfs/internal/hash"
	"git-sfs/internal/sfspath"
)

// Mv moves a git-sfs symlink or directory of symlinks to dst, rewriting
// relative targets for the new location. The cache is not touched.
func (a App) Mv(src, dst string) (err error) {
	a.debugf("mv: start src=%s dst=%s", src, dst)
	defer a.debugDone("mv", &err)
	repo, _, _, err := a.open()
	if err != nil {
		return err
	}
	srcAbs := absFromRepo(repo, src)
	dstAbs := absFromRepo(repo, dst)

	info, err := os.Lstat(srcAbs)
	if err != nil {
		return fmt.Errorf("mv: %w", err)
	}
	if info.IsDir() {
		return a.mvDir(repo, srcAbs, dstAbs)
	}
	return a.mvLink(repo, srcAbs, dstAbs)
}

func (a App) mvLink(repo, srcAbs, dstAbs string) error {
	h, _, err := sfspath.ParseGitSymlink(repo, srcAbs)
	if err != nil {
		return fmt.Errorf("mv: %s is not a git-sfs symlink: %w", srcAbs, err)
	}
	// POSIX: if dst is an existing directory, place the file inside it.
	if info, err := os.Lstat(dstAbs); err == nil && info.IsDir() {
		dstAbs = filepath.Join(dstAbs, filepath.Base(srcAbs))
	}
	if _, err := os.Lstat(dstAbs); err == nil {
		return fmt.Errorf("mv: destination already exists: %s", dstAbs)
	}
	if err := os.MkdirAll(filepath.Dir(dstAbs), 0o755); err != nil {
		return err
	}
	target, err := sfspath.GitLinkTarget(repo, dstAbs, h)
	if err != nil {
		return err
	}
	if err := os.Symlink(target, dstAbs); err != nil {
		return err
	}
	if err := os.Remove(srcAbs); err != nil {
		_ = os.Remove(dstAbs)
		return err
	}
	a.say("moved " + rel(repo, srcAbs) + " -> " + rel(repo, dstAbs))
	return nil
}

func (a App) mvDir(repo, srcAbs, dstAbs string) error {
	// POSIX: if dst already exists as a directory, place src inside it.
	if info, err := os.Lstat(dstAbs); err == nil && info.IsDir() {
		dstAbs = filepath.Join(dstAbs, filepath.Base(srcAbs))
	}

	// Collect git-sfs symlinks before renaming — ParseGitSymlink needs
	// the symlink at its original location to validate relative targets.
	type entry struct {
		relPath string
		h       hash.Hash
	}
	var links []entry
	if err := filepath.WalkDir(srcAbs, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.Type()&os.ModeSymlink == 0 {
			return err
		}
		h, _, parseErr := sfspath.ParseGitSymlink(repo, path)
		if parseErr != nil {
			return nil // skip non-git-sfs symlinks
		}
		relPath, _ := filepath.Rel(srcAbs, path)
		links = append(links, entry{relPath, h})
		return nil
	}); err != nil {
		return err
	}

	if err := os.MkdirAll(filepath.Dir(dstAbs), 0o755); err != nil {
		return err
	}
	if err := os.Rename(srcAbs, dstAbs); err != nil {
		return fmt.Errorf("mv: %w", err)
	}

	// Rewrite symlink targets now that the whole tree is at the new location.
	for _, e := range links {
		newPath := filepath.Join(dstAbs, e.relPath)
		target, err := sfspath.GitLinkTarget(repo, newPath, e.h)
		if err != nil {
			return err
		}
		if err := os.Remove(newPath); err != nil {
			return err
		}
		if err := os.Symlink(target, newPath); err != nil {
			return err
		}
		oldPath := filepath.Join(srcAbs, e.relPath)
		a.say("moved " + rel(repo, oldPath) + " -> " + rel(repo, newPath))
	}
	return nil
}
