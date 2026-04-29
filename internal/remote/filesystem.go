package remote

import (
	"context"
	"os"
	"path/filepath"

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
	select {
	case <-ctx.Done():
		return false, ctx.Err()
	default:
	}
	return hash.VerifyFile(r.path(h), h) == nil, nil
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
		return nil
	}
	if err := fsutil.AtomicCopy(srcPath, dst, 0o644); err != nil {
		return err
	}
	return hash.VerifyFile(dst, h)
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
	if err := fsutil.AtomicCopy(r.path(h), dstPath, 0o644); err != nil {
		return err
	}
	return hash.VerifyFile(dstPath, h)
}
