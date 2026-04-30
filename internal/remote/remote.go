package remote

import (
	"context"
	"fmt"
	"io"
	"path/filepath"

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
	Debug        io.Writer
	ConfigDir    string
	RcloneConfig string
}

func New(cfg config.RemoteConfig) (Remote, error) {
	return NewWithOptions(cfg, Options{})
}

func NewWithOptions(cfg config.RemoteConfig, opts Options) (Remote, error) {
	switch cfg.Type {
	case "filesystem", "fs":
		return NewFilesystem(remotePathConfig(cfg)), nil
	case "rclone":
		opts.RcloneConfig = rcloneConfigPath(opts.ConfigDir, cfg.Config)
		if cfg.Host != "" || cfg.Path != "" {
			return NewRcloneTargetWithOptions(cfg.Host, cfg.Path, opts), nil
		}
		return NewRcloneWithOptions(cfg.URL, opts), nil
	default:
		return nil, fmt.Errorf("unsupported remote type %q", cfg.Type)
	}
}

func rcloneConfigPath(configDir, config string) string {
	if config == "" || filepath.IsAbs(config) {
		return config
	}
	return filepath.Join(configDir, config)
}

func remotePathConfig(cfg config.RemoteConfig) string {
	if cfg.Path != "" {
		return cfg.Path
	}
	return cfg.URL
}
