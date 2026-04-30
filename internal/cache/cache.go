package cache

import (
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
