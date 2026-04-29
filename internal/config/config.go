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
	if err := os.WriteFile(path, []byte(defaultYAML), 0o644); err != nil {
		return fmt.Errorf("write %s: %w", path, err)
	}
	return nil
}

const defaultYAML = `version: 1

remotes:
  default:
    type: rsync
    url: user@host:/mnt/datasets/project

settings:
  algorithm: sha256
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
		indent := len(raw) - len(strings.TrimLeft(raw, " "))
		line := strings.TrimSpace(raw)
		if indent == 0 {
			key, val, ok := field(line)
			if !ok {
				return Config{}, fmt.Errorf("invalid config line %q", line)
			}
			switch key {
			case "version":
				if val != "1" {
					return Config{}, fmt.Errorf("unsupported dataset.yaml version %q", val)
				}
				cfg.Version = Version
			case "remotes", "settings":
				section = key
				remote = ""
			case "cache":
				return Config{}, fmt.Errorf("dataset.yaml must not contain local cache configuration")
			default:
				return Config{}, fmt.Errorf("unknown dataset.yaml field %q", key)
			}
			continue
		}
		if section == "remotes" && indent == 2 {
			name := strings.TrimSuffix(line, ":")
			if name == "" || name == line {
				return Config{}, fmt.Errorf("invalid remote declaration %q", line)
			}
			remote = name
			cfg.Remotes[remote] = RemoteConfig{}
			continue
		}
		key, val, ok := field(line)
		if !ok {
			return Config{}, fmt.Errorf("invalid config line %q", line)
		}
		switch section {
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
		default:
			return Config{}, fmt.Errorf("field %q outside a known section", key)
		}
	}
	if err := sc.Err(); err != nil {
		return Config{}, err
	}
	if cfg.Version != Version {
		return Config{}, fmt.Errorf("dataset.yaml must declare version: 1")
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
	path := filepath.Join(repo, ".ds", "local.yaml")
	f, err := os.Open(path)
	if os.IsNotExist(err) {
		return Local{}, nil
	}
	if err != nil {
		return Local{}, err
	}
	defer f.Close()

	var local Local
	var section string
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		raw := stripComment(sc.Text())
		if strings.TrimSpace(raw) == "" {
			continue
		}
		indent := len(raw) - len(strings.TrimLeft(raw, " "))
		line := strings.TrimSpace(raw)
		if indent == 0 {
			key, _, ok := field(line)
			if !ok {
				return Local{}, fmt.Errorf("invalid local config line %q", line)
			}
			if key != "cache" {
				return Local{}, fmt.Errorf("unknown .ds/local.yaml field %q", key)
			}
			section = key
			continue
		}
		key, val, ok := field(line)
		if !ok {
			return Local{}, fmt.Errorf("invalid local config line %q", line)
		}
		if section == "cache" && key == "path" {
			local.CachePath = val
		}
	}
	return local, sc.Err()
}

func field(line string) (string, string, bool) {
	parts := strings.SplitN(line, ":", 2)
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
