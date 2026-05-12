package cache

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"

	"git-sfs/internal/fsutil"
	"git-sfs/internal/hash"
)

type Cache struct {
	Root string
}

// FilePath returns the deterministic content-addressed location for h.
func (c Cache) FilePath(h hash.Hash) string {
	return filepath.Join(c.Root, "files", hash.Algorithm, h.Prefix(), h.String())
}

func (c Cache) TmpDir() string   { return filepath.Join(c.Root, "tmp") }
func (c Cache) LocksDir() string { return filepath.Join(c.Root, "locks") }

func (c Cache) Init() error {
	for _, p := range []string{
		filepath.Join(c.Root, "files", hash.Algorithm),
		c.TmpDir(),
		c.LocksDir(),
	} {
		if err := fsutil.EnsureDir(p); err != nil {
			return err
		}
	}
	return nil
}

// HasValid reports whether h is present in the cache and intact.
// A read-only file at the content-addressed path is sufficient proof: write
// protection is set by Protect only after hash verification, so the file cannot
// have been mutated since. Files that predate write-protection enforcement are
// hash-verified on first access and protected in place as a one-time migration.
func (c Cache) HasValid(h hash.Hash) bool {
	path := c.FilePath(h)
	info, err := os.Stat(path)
	if err != nil {
		return false
	}
	if info.Mode()&0o222 == 0 {
		return true
	}
	// Legacy file: has write bit, so verify by hash and protect if valid.
	if hash.VerifyFile(path, h) != nil {
		return false
	}
	os.Chmod(path, readOnly(info.Mode()))
	return true
}

// readOnly strips write bits from mode, preserving read and execute bits.
func readOnly(mode os.FileMode) os.FileMode {
	return mode &^ 0o222
}

func (c Cache) Protect(h hash.Hash) error {
	path := c.FilePath(h)
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	// Already protected: write-protection was set only after prior hash verification,
	// so the file cannot have been mutated since. No need to re-hash.
	if info.Mode()&0o222 == 0 {
		return nil
	}
	if err := hash.VerifyFile(path, h); err != nil {
		return err
	}
	return os.Chmod(path, readOnly(info.Mode()))
}

// Store copies src into the cache only after naming it by its expected hash.
// The final file is accepted only if its bytes still match h.
func (c Cache) Store(src string, h hash.Hash) error {
	st, err := os.Stat(src)
	if err != nil {
		return err
	}
	mode := readOnly(st.Mode())
	dst := c.FilePath(h)
	if c.HasValid(h) {
		return os.Chmod(dst, mode)
	}
	if err := fsutil.AtomicCopy(src, dst, mode); err != nil {
		return err
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		return err
	}
	return os.Chmod(dst, mode)
}

// Move moves src into the cache, verifies it by hash, then publishes the final immutable object.
func (c Cache) Move(src string, h hash.Hash) error {
	dst := c.FilePath(h)
	srcAbs, err := filepath.Abs(src)
	if err != nil {
		return err
	}
	st, err := os.Stat(src)
	if err != nil {
		return err
	}
	mode := readOnly(st.Mode())
	if filepath.Clean(srcAbs) == filepath.Clean(dst) {
		return os.Chmod(dst, mode)
	}
	if c.HasValid(h) {
		if err := os.Chmod(dst, mode); err != nil {
			return err
		}
		return os.Remove(src)
	}
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		return err
	}
	if err := os.MkdirAll(c.TmpDir(), 0o755); err != nil {
		return err
	}
	tmp := filepath.Join(c.TmpDir(), "."+h.String()+".move")
	_ = os.Remove(tmp)
	if err := os.Rename(src, tmp); err != nil {
		if !isCrossDeviceRename(err) {
			return fmt.Errorf("move into cache staging failed: %w", err)
		}
		if err := copyThenRemove(src, tmp, mode); err != nil {
			return err
		}
	}
	if err := hash.VerifyFile(tmp, h); err != nil {
		return err
	}
	if err := os.Chmod(tmp, mode); err != nil {
		return err
	}
	if err := os.Rename(tmp, dst); err != nil {
		return fmt.Errorf("publish cached file %s: %w", dst, err)
	}
	return os.Chmod(dst, mode)
}

func isCrossDeviceRename(err error) bool {
	return errors.Is(err, syscall.EXDEV)
}

func copyThenRemove(src, dst string, mode os.FileMode) error {
	if err := fsutil.AtomicCopy(src, dst, mode); err != nil {
		return fmt.Errorf("copy into cache staging after cross-filesystem rename failed: %w", err)
	}
	if err := os.Remove(src); err != nil {
		return fmt.Errorf("remove source after cross-filesystem cache copy: %w", err)
	}
	return nil
}
