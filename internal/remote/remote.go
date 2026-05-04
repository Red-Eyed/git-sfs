package remote

import (
	"context"
	"io"
	"path/filepath"

	"git-sfs/internal/config"
	"git-sfs/internal/hash"
)

// Remote hides backend details from push and pull workflow code.
type Remote interface {
	// Ping does a lightweight check that the remote is reachable.
	Ping(ctx context.Context) error
	HasFile(ctx context.Context, h hash.Hash) (bool, error)
	CheckFile(ctx context.Context, h hash.Hash) (bool, error)
	PushFile(ctx context.Context, h hash.Hash, srcPath string) error
	PullFile(ctx context.Context, h hash.Hash, dstPath string) error
}

type Options struct {
	Debug        io.Writer
	ConfigDir    string
	RcloneConfig string
	RetryMax     int
}

func New(cfg config.RemoteConfig) (Remote, error) {
	return NewWithOptions(cfg, Options{})
}

func NewWithOptions(cfg config.RemoteConfig, opts Options) (Remote, error) {
	opts.RcloneConfig = rcloneConfigPath(opts.ConfigDir, cfg.Config)
	return NewRcloneTargetWithOptions(cfg.Backend, cfg.Path, opts), nil
}

func rcloneConfigPath(configDir, config string) string {
	if config == "" || filepath.IsAbs(config) {
		return config
	}
	return filepath.Join(configDir, config)
}
