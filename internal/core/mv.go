package core

import (
	"fmt"
	"os"
	"path/filepath"

	"git-sfs/internal/sfspath"
)

// Mv moves a git-sfs symlink from src to dst, rewriting the relative target
// for the new location. The cache is not touched.
func (a App) Mv(src, dst string) (err error) {
	a.debugf("mv: start src=%s dst=%s", src, dst)
	defer a.debugDone("mv", &err)
	repo, _, _, err := a.open()
	if err != nil {
		return err
	}
	srcAbs := absFromRepo(repo, src)
	h, _, err := sfspath.ParseGitSymlink(repo, srcAbs)
	if err != nil {
		return fmt.Errorf("mv: %s is not a git-sfs symlink: %w", src, err)
	}
	dstAbs := absFromRepo(repo, dst)
	// If dst is a directory, place the file inside it (POSIX mv convention).
	if info, err := os.Lstat(dstAbs); err == nil && info.IsDir() {
		dstAbs = filepath.Join(dstAbs, filepath.Base(srcAbs))
	}
	if _, err := os.Lstat(dstAbs); err == nil {
		return fmt.Errorf("mv: destination already exists: %s", dst)
	}
	target, err := sfspath.GitLinkTarget(repo, dstAbs, h)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(dstAbs), 0o755); err != nil {
		return err
	}
	if err := os.Symlink(target, dstAbs); err != nil {
		return err
	}
	if err := os.Remove(srcAbs); err != nil {
		_ = os.Remove(dstAbs) // roll back
		return err
	}
	a.say("moved " + rel(repo, srcAbs) + " -> " + rel(repo, dstAbs))
	return nil
}
