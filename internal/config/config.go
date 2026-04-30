package config

import (
	"bufio"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"git-sfs/internal/errs"
)

const Version = 1

type Config struct {
	Version  int
	Remotes  map[string]RemoteConfig
	Settings Settings
}

type RemoteConfig struct {
	Type   string
	URL    string
	Host   string
	Path   string
	Config string
}

type Settings struct {
	Algorithm string
}

type Local struct {
	CachePath string
}

func Default() Config {
	return Config{
		Version: Version,
		Remotes: map[string]RemoteConfig{
			"default": {Type: "rclone", Host: "remote-name", Path: "datasets/project", Config: "rclone.conf"},
		},
		Settings: Settings{Algorithm: "sha256"},
	}
}

func WriteDefault(path string) error {
	if err := os.WriteFile(path, []byte(defaultTOML), 0o644); err != nil {
		return fmt.Errorf("write %s: %w", path, err)
	}
	return nil
}

const defaultTOML = `# git-sfs project config. Commit this file to Git.
# Do not put local cache paths, secrets, tokens, or machine-specific paths here.

version = 1

# The default remote is used by git-sfs push and git-sfs pull when no remote is named.
[remotes.default]
# Supported today: rclone and filesystem.
type = "rclone"
host = "remote-name"
path = "datasets/project"
# Relative paths are resolved from .git-sfs.
# Do not commit rclone configs that contain secrets or tokens.
config = "rclone.conf"

# Examples you can copy by removing the leading # characters.
# [remotes.local]
# type = "filesystem"
# path = "/mnt/datasets/project"

[settings]
# Only sha256 is supported in v1.
algorithm = "sha256"
`

func Load(path string) (Config, error) {
	f, err := os.Open(path)
	if err != nil {
		return Config{}, fmt.Errorf("open config %s: %w", path, err)
	}
	defer f.Close()

	cfg := Config{Remotes: map[string]RemoteConfig{}}
	var section, remote string
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		raw := stripComment(sc.Text())
		if strings.TrimSpace(raw) == "" {
			continue
		}
		line := strings.TrimSpace(raw)
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			name := strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(line, "["), "]"))
			switch {
			case name == "settings":
				section = "settings"
				remote = ""
			case strings.HasPrefix(name, "remotes."):
				section = "remotes"
				remote = strings.TrimPrefix(name, "remotes.")
				if remote == "" {
					return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("invalid remote section %q", name))
				}
				cfg.Remotes[remote] = RemoteConfig{}
			case name == "cache" || strings.HasPrefix(name, "cache."):
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf(".git-sfs/config.toml must not contain local cache configuration"))
			default:
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unknown .git-sfs/config.toml section %q", name))
			}
			continue
		}
		key, val, ok := field(line)
		if !ok {
			return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("invalid config line %q", line))
		}
		switch section {
		case "":
			if key == "version" {
				if val != "1" {
					return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unsupported .git-sfs/config.toml version %q", val))
				}
				cfg.Version = Version
				continue
			}
			if key == "cache" {
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf(".git-sfs/config.toml must not contain local cache configuration"))
			}
			return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unknown .git-sfs/config.toml field %q", key))
		case "settings":
			if key != "algorithm" {
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unknown settings field %q", key))
			}
			cfg.Settings.Algorithm = val
		case "remotes":
			if remote == "" {
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("remote field %q appears before remote name", key))
			}
			rc := cfg.Remotes[remote]
			switch key {
			case "type":
				rc.Type = val
			case "url":
				rc.URL = val
			case "host":
				rc.Host = val
			case "path":
				rc.Path = val
			case "config":
				rc.Config = val
			default:
				return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unknown remote field %q", key))
			}
			cfg.Remotes[remote] = rc
		}
	}
	if err := sc.Err(); err != nil {
		return Config{}, err
	}
	if cfg.Version != Version {
		return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf(".git-sfs/config.toml must declare version = 1"))
	}
	if cfg.Settings.Algorithm == "" {
		cfg.Settings.Algorithm = "sha256"
	}
	if cfg.Settings.Algorithm != "sha256" {
		return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("unsupported hash algorithm %q", cfg.Settings.Algorithm))
	}
	for name, rc := range cfg.Remotes {
		if rc.Type == "" {
			return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("remote %q requires type", name))
		}
		if rc.URL == "" && rc.Path == "" {
			return Config{}, errors.Join(errs.ErrInvalidConfig, fmt.Errorf("remote %q requires url or path", name))
		}
	}
	return cfg, nil
}

func LoadLocal(repo string) (Local, error) {
	path := filepath.Join(repo, ".git-sfs", "cache")
	target, err := os.Readlink(path)
	if os.IsNotExist(err) {
		return Local{}, nil
	}
	if err != nil {
		return Local{}, fmt.Errorf("read cache link %s: %w", path, err)
	}
	if !filepath.IsAbs(target) {
		target = filepath.Join(filepath.Dir(path), target)
	}
	return Local{CachePath: target}, nil
}

func field(line string) (string, string, bool) {
	parts := strings.SplitN(line, "=", 2)
	if len(parts) != 2 {
		return "", "", false
	}
	return strings.TrimSpace(parts[0]), unquote(strings.TrimSpace(parts[1])), true
}

func unquote(s string) string {
	s = strings.TrimSpace(s)
	return strings.Trim(s, `"'`)
}

func stripComment(s string) string {
	if i := strings.IndexByte(s, '#'); i >= 0 {
		return s[:i]
	}
	return s
}
