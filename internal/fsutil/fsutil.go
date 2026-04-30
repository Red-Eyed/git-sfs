package fsutil

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
)

// AtomicCopy writes into the destination directory first, then renames into
// place so interrupted copies do not publish partial final files.
func AtomicCopy(src, dst string, mode os.FileMode) error {
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(dst), ".git-sfs-tmp-*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)

	in, err := os.Open(src)
	if err != nil {
		tmp.Close()
		return err
	}
	defer in.Close()
	if _, err := io.Copy(tmp, in); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Chmod(mode); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmpName, dst)
}

func ReadOnlyMode(mode os.FileMode) os.FileMode {
	return mode &^ 0o222
}

func MakeReadOnly(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	return os.Chmod(path, ReadOnlyMode(info.Mode().Perm()))
}

// RelSymlink creates a symlink whose target is relative to the link location.
func RelSymlink(target, link string) error {
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		return err
	}
	rel, err := filepath.Rel(filepath.Dir(link), target)
	if err != nil {
		return err
	}
	_ = os.Remove(link)
	return os.Symlink(rel, link)
}

// AbsoluteSymlink is used only for local untracked state under .git-sfs/cache.
func AbsoluteSymlink(target, link string) error {
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		return err
	}
	_ = os.Remove(link)
	return os.Symlink(target, link)
}

func IsInside(root, path string) bool {
	rel, err := filepath.Rel(root, path)
	return err == nil && rel != "." && rel != ".." && len(rel) >= 2 && rel[:2] != ".."
}

func EnsureDir(path string) error {
	if err := os.MkdirAll(path, 0o755); err != nil {
		return fmt.Errorf("create %s: %w", path, err)
	}
	return nil
}
