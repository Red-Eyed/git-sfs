package fsutil

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
)

const chunkSize = 4 << 20

// CopyCtx copies from src to dst in chunks, checking ctx before each read so a
// long copy can be canceled promptly (within one chunk). It returns the number
// of bytes written. Unlike io.Copy it never uses ReadFrom/WriteTo fast paths,
// which would bypass the cancellation check.
func CopyCtx(ctx context.Context, dst io.Writer, src io.Reader, buf []byte) (int64, error) {
	var written int64
	for {
		if err := ctx.Err(); err != nil {
			return written, err
		}
		n, readErr := src.Read(buf)
		if n > 0 {
			w, writeErr := dst.Write(buf[:n])
			written += int64(w)
			if writeErr != nil {
				return written, writeErr
			}
			if w < n {
				return written, io.ErrShortWrite
			}
		}
		if readErr == io.EOF {
			return written, nil
		}
		if readErr != nil {
			return written, readErr
		}
	}
}

// AtomicCopy writes into the destination directory first, then renames into
// place so interrupted copies do not publish partial final files. The copy is
// cancelable via ctx; on cancellation the temp file is removed by the deferred
// cleanup, so no partial bytes are published.
func AtomicCopy(ctx context.Context, src, dst string, mode os.FileMode) error {
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
	buf := make([]byte, chunkSize)
	if _, err := CopyCtx(ctx, tmp, in, buf); err != nil {
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
