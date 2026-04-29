package remote

import (
	"context"
	"fmt"

	"github.com/vadstup/merk/internal/config"
	"github.com/vadstup/merk/internal/hash"
)

// Remote hides backend details from push and pull workflow code.
type Remote interface {
	HasFile(ctx context.Context, h hash.Hash) (bool, error)
	PushFile(ctx context.Context, h hash.Hash, srcPath string) error
	PullFile(ctx context.Context, h hash.Hash, dstPath string) error
}

func New(cfg config.RemoteConfig) (Remote, error) {
	switch cfg.Type {
	case "filesystem", "fs":
		return NewFilesystem(cfg.URL), nil
	case "rsync":
		return NewRsync(cfg.URL), nil
	case "ssh":
		return NewSSH(cfg.URL), nil
	default:
		return nil, fmt.Errorf("unsupported remote type %q", cfg.Type)
	}
}
