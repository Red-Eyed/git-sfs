package remote

import (
	"context"
	"fmt"
	"io"

	"git-sfs/internal/config"
	"git-sfs/internal/hash"
)

// Remote hides backend details from push and pull workflow code.
type Remote interface {
	HasFile(ctx context.Context, h hash.Hash) (bool, error)
	PushFile(ctx context.Context, h hash.Hash, srcPath string) error
	PullFile(ctx context.Context, h hash.Hash, dstPath string) error
}

type Options struct {
	Debug io.Writer
	Shell string
}

func New(cfg config.RemoteConfig) (Remote, error) {
	return NewWithOptions(cfg, Options{})
}

func NewWithOptions(cfg config.RemoteConfig, opts Options) (Remote, error) {
	switch cfg.Type {
	case "filesystem", "fs":
		return NewFilesystem(remotePathConfig(cfg)), nil
	case "rsync":
		opts.Shell = rcShell(cfg)
		if cfg.Host != "" || cfg.Path != "" {
			return NewRsyncTargetWithOptions(cfg.Host, cfg.Path, opts), nil
		}
		return NewRsyncWithOptions(cfg.URL, opts), nil
	case "ssh":
		opts.Shell = rcShell(cfg)
		if cfg.Host != "" || cfg.Path != "" {
			return NewSSHTargetWithOptions(cfg.Host, cfg.Path, opts), nil
		}
		return NewSSHWithOptions(cfg.URL, opts), nil
	case "rclone":
		if cfg.Host != "" || cfg.Path != "" {
			return NewRcloneTargetWithOptions(cfg.Host, cfg.Path, opts), nil
		}
		return NewRcloneWithOptions(cfg.URL, opts), nil
	default:
		return nil, fmt.Errorf("unsupported remote type %q", cfg.Type)
	}
}

func rcShell(cfg config.RemoteConfig) string {
	if cfg.Shell != "" {
		return cfg.Shell
	}
	return "sh"
}

func remotePathConfig(cfg config.RemoteConfig) string {
	if cfg.Path != "" {
		return cfg.Path
	}
	return cfg.URL
}
