package remote

import (
	"context"
	"io"

	"git-sfs/internal/hash"
	"git-sfs/internal/progress"
)

// progressRemote wraps a Remote and adds live spinner feedback for slow
// metadata calls (lsjson). Transfer calls (CopyToRemote, CopyFromRemote)
// are delegated unchanged because rclone renders its own --progress bar.
type progressRemote struct {
	inner  Remote
	stderr io.Writer
}

// WithProgress wraps r so that slow metadata calls display a spinner on
// stderr while they run. If quiet is true or stderr is nil, r is returned
// as-is.
func WithProgress(r Remote, stderr io.Writer, quiet bool) Remote {
	if quiet || stderr == nil {
		return r
	}
	return progressRemote{inner: r, stderr: stderr}
}

func (p progressRemote) spin(label string) func() {
	s := progress.NewSpinner(p.stderr, label, false)
	return s.Stop
}

func (p progressRemote) CheckBackend(ctx context.Context) error {
	stop := p.spin("connecting to remote")
	err := p.inner.CheckBackend(ctx)
	stop()
	return err
}

func (p progressRemote) CheckPath(ctx context.Context) error {
	stop := p.spin("connecting to remote")
	err := p.inner.CheckPath(ctx)
	stop()
	return err
}

func (p progressRemote) RequireExists(ctx context.Context) error {
	stop := p.spin("connecting to remote")
	err := p.inner.RequireExists(ctx)
	stop()
	return err
}

func (p progressRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	stop := p.spin("querying remote")
	ok, err := p.inner.HasFile(ctx, h)
	stop()
	return ok, err
}

func (p progressRemote) CheckFile(ctx context.Context, h hash.Hash) (bool, error) {
	stop := p.spin("verifying remote file")
	ok, err := p.inner.CheckFile(ctx, h)
	stop()
	return ok, err
}

func (p progressRemote) FileSize(ctx context.Context, h hash.Hash) (int64, error) {
	stop := p.spin("querying remote")
	n, err := p.inner.FileSize(ctx, h)
	stop()
	return n, err
}

func (p progressRemote) FileSizes(ctx context.Context, hashes []hash.Hash) (map[hash.Hash]int64, error) {
	stop := p.spin("querying remote")
	m, err := p.inner.FileSizes(ctx, hashes)
	stop()
	return m, err
}

func (p progressRemote) CopyToRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error {
	return p.inner.CopyToRemote(ctx, cacheFilesDir, relPaths)
}

func (p progressRemote) CopyFromRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error {
	return p.inner.CopyFromRemote(ctx, cacheFilesDir, relPaths)
}
