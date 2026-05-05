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
	// CheckBackend verifies that the rclone backend itself is reachable,
	// without checking whether the configured path exists.
	CheckBackend(ctx context.Context) error
	// CheckPath verifies that the configured remote path exists.
	// Call after CheckBackend to distinguish connectivity from missing-path errors.
	CheckPath(ctx context.Context) error
	// RequireExists checks backend connectivity and that the root path exists.
	// Returns an error if the backend is unreachable or the path is missing.
	RequireExists(ctx context.Context) error
	HasFile(ctx context.Context, h hash.Hash) (bool, error)
	CheckFile(ctx context.Context, h hash.Hash) (bool, error)
	// FileSize returns the byte size of the remote file for h, or -1 if not found.
	FileSize(ctx context.Context, h hash.Hash) (int64, error)
	// CopyToRemote uploads files listed by relPaths (relative to cacheFilesDir)
	// to the remote files directory. Existing remote files are never overwritten.
	CopyToRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error
	// CopyFromRemote downloads files listed by relPaths (relative to the remote
	// files directory) into cacheFilesDir. Existing local files are preserved.
	CopyFromRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error
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

// ResolveConfigPath resolves a rclone config file path relative to configDir.
// Absolute paths and empty strings are returned as-is.
func ResolveConfigPath(configDir, cfgPath string) string {
	if cfgPath == "" || filepath.IsAbs(cfgPath) {
		return cfgPath
	}
	return filepath.Join(configDir, cfgPath)
}

func rcloneConfigPath(configDir, config string) string {
	return ResolveConfigPath(configDir, config)
}
