package cache

import (
	"fmt"
	"os"
	"path/filepath"

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

func (c Cache) HasValid(h hash.Hash) bool {
	return hash.VerifyFile(c.FilePath(h), h) == nil
}

func (c Cache) Protect(h hash.Hash) error {
	path := c.FilePath(h)
	if err := hash.VerifyFile(path, h); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(path)
}

// Store copies src into the cache only after naming it by its expected hash.
// The final file is accepted only if its bytes still match h.
func (c Cache) Store(src string, h hash.Hash) error {
	dst := c.FilePath(h)
	if c.HasValid(h) {
		return fsutil.MakeReadOnly(dst)
	}
	st, err := os.Stat(src)
	if err != nil {
		return err
	}
	if err := fsutil.AtomicCopy(src, dst, fsutil.ReadOnlyMode(st.Mode().Perm())); err != nil {
		return err
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(dst)
}

// Move renames src into the cache, verifies it by hash, then publishes the final immutable object.
func (c Cache) Move(src string, h hash.Hash) error {
	dst := c.FilePath(h)
	srcAbs, err := filepath.Abs(src)
	if err != nil {
		return err
	}
	if filepath.Clean(srcAbs) == filepath.Clean(dst) {
		return fsutil.MakeReadOnly(dst)
	}
	if c.HasValid(h) {
		if err := fsutil.MakeReadOnly(dst); err != nil {
			return err
		}
		return os.Remove(src)
	}
	st, err := os.Stat(src)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		return err
	}
	if err := os.MkdirAll(c.TmpDir(), 0o755); err != nil {
		return err
	}
	if err := os.Chmod(src, fsutil.ReadOnlyMode(st.Mode().Perm())); err != nil {
		return err
	}
	tmp := filepath.Join(c.TmpDir(), "."+h.String()+".move")
	_ = os.Remove(tmp)
	if err := os.Rename(src, tmp); err != nil {
		return fmt.Errorf("move into cache staging failed; source and cache must be on the same filesystem: %w", err)
	}
	if err := hash.VerifyFile(tmp, h); err != nil {
		return err
	}
	if err := os.Chmod(tmp, fsutil.ReadOnlyMode(st.Mode().Perm())); err != nil {
		return err
	}
	if err := os.Rename(tmp, dst); err != nil {
		return fmt.Errorf("publish cached file %s: %w", dst, err)
	}
	return nil
}
