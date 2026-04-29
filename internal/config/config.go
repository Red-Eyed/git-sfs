package config

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const Version = 1

type Config struct {
	Version  int
	Remotes  map[string]RemoteConfig
	Settings Settings
}

type RemoteConfig struct {
	Type string
	URL  string
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
			"default": {Type: "rsync", URL: "user@host:/mnt/datasets/project"},
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

const defaultTOML = `# merk project config. Commit this file to Git.
# Do not put local cache paths, secrets, tokens, or machine-specific paths here.

version = 1

# The default remote is used by merk push and merk pull when no remote is named.
[remotes.default]
# Supported today: rsync, ssh, filesystem.
# Use rsync for a normal host:path destination.
type = "rsync"
url = "user@host:/mnt/datasets/project"

# Examples you can copy by removing the leading # characters.
# [remotes.backup]
# type = "ssh"
# url = "user@host:/mnt/datasets/project"
#
# [remotes.local]
# type = "filesystem"
# url = "/mnt/datasets/project"

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
					return Config{}, fmt.Errorf("invalid remote section %q", name)
				}
				cfg.Remotes[remote] = RemoteConfig{}
			case name == "cache" || strings.HasPrefix(name, "cache."):
				return Config{}, fmt.Errorf(".merk/config.toml must not contain local cache configuration")
			default:
				return Config{}, fmt.Errorf("unknown .merk/config.toml section %q", name)
			}
			continue
		}
		key, val, ok := field(line)
		if !ok {
			return Config{}, fmt.Errorf("invalid config line %q", line)
		}
		switch section {
		case "":
			if key == "version" {
				if val != "1" {
					return Config{}, fmt.Errorf("unsupported .merk/config.toml version %q", val)
				}
				cfg.Version = Version
				continue
			}
			if key == "cache" {
				return Config{}, fmt.Errorf(".merk/config.toml must not contain local cache configuration")
			}
			return Config{}, fmt.Errorf("unknown .merk/config.toml field %q", key)
		case "settings":
			if key != "algorithm" {
				return Config{}, fmt.Errorf("unknown settings field %q", key)
			}
			cfg.Settings.Algorithm = val
		case "remotes":
			if remote == "" {
				return Config{}, fmt.Errorf("remote field %q appears before remote name", key)
			}
			rc := cfg.Remotes[remote]
			switch key {
			case "type":
				rc.Type = val
			case "url":
				rc.URL = val
			default:
				return Config{}, fmt.Errorf("unknown remote field %q", key)
			}
			cfg.Remotes[remote] = rc
		}
	}
	if err := sc.Err(); err != nil {
		return Config{}, err
	}
	if cfg.Version != Version {
		return Config{}, fmt.Errorf(".merk/config.toml must declare version = 1")
	}
	if cfg.Settings.Algorithm == "" {
		cfg.Settings.Algorithm = "sha256"
	}
	if cfg.Settings.Algorithm != "sha256" {
		return Config{}, fmt.Errorf("unsupported hash algorithm %q", cfg.Settings.Algorithm)
	}
	for name, rc := range cfg.Remotes {
		if rc.Type == "" || rc.URL == "" {
			return Config{}, fmt.Errorf("remote %q requires type and url", name)
		}
	}
	return cfg, nil
}

func LoadLocal(repo string) (Local, error) {
	path := filepath.Join(repo, ".merk", "cache")
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
