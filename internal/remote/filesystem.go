package remote

import (
	"context"
	"errors"
	"os"
	"path/filepath"

	"git-sfs/internal/errs"
	"git-sfs/internal/fsutil"
	"git-sfs/internal/hash"
)

type filesystemRemote struct {
	root string
}

func NewFilesystem(root string) Remote {
	return filesystemRemote{root: root}
}

func (r filesystemRemote) path(h hash.Hash) string {
	return filepath.Join(r.root, "files", hash.Algorithm, h.Prefix(), h.String())
}

func (r filesystemRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	ok, err := r.CheckFile(ctx, h)
	return ok, err
}

func (r filesystemRemote) CheckFile(ctx context.Context, h hash.Hash) (bool, error) {
	select {
	case <-ctx.Done():
		return false, ctx.Err()
	default:
	}
	if err := hash.VerifyFile(r.path(h), h); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return false, nil
		}
		return false, errors.Join(errs.ErrCorruptRemoteFile, err)
	}
	return true, nil
}

// PushFile treats an existing valid file as success, which makes retries
// cheap after interrupted or duplicated uploads.
func (r filesystemRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
	}
	dst := r.path(h)
	if hash.VerifyFile(dst, h) == nil {
		return fsutil.MakeReadOnly(dst)
	}
	if err := fsutil.AtomicCopy(srcPath, dst, fsutil.ReadOnlyMode(0o644)); err != nil {
		return err
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(dst)
}

// PullFile verifies the source before copying and verifies the destination
// again before returning success.
func (r filesystemRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
	}
	if err := hash.VerifyFile(r.path(h), h); err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(dstPath), 0o755); err != nil {
		return err
	}
	if err := fsutil.AtomicCopy(r.path(h), dstPath, fsutil.ReadOnlyMode(0o644)); err != nil {
		return err
	}
	if err := hash.VerifyFile(dstPath, h); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(dstPath)
}
