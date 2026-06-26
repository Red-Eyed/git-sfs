package core

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func app(stdout *bytes.Buffer) App {
	return App{
		Stdout:     stdout,
		Stderr:     &bytes.Buffer{},
		ConfigPath: ".git-sfs/config.toml",
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	return repo
}

func writeRcloneDataset(t *testing.T, repo, remote, path string) {
	t.Helper()
	content := "version = 1\n\n[remotes.default]\nbackend = " + remote + "\npath = " + path + "\n\n[settings]\nalgorithm = sha256\n"
	mustWrite(t, filepath.Join(repo, ".git-sfs/config.toml"), []byte(content))
}

func writeLocal(t *testing.T, repo, cacheDir string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(repo, ".git-sfs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(cacheDir, filepath.Join(repo, ".git-sfs", "cache")); err != nil {
		t.Fatal(err)
	}
}

func mustWrite(t *testing.T, path string, content []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, content, 0o644); err != nil {
		t.Fatal(err)
	}
}

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}

func writeTimedRcloneTool(t *testing.T, path string) {
	t.Helper()
	writeTool(t, path, `set -eu
if [ "${1:-}" = "--config" ]; then
  shift 2
fi
if [ "${1:-}" = "--progress" ]; then shift; fi
cmd="${1:-}"
map_path() {
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
case "$cmd" in
  copyto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    case "$src" in
      "$RCLONE_TEST_ROOT"/*)
        printf 'start %s\n' "$src" >> "$RCLONE_TEST_LOG"
        sleep 1
        cp "$src" "$dst"
        printf 'end %s\n' "$src" >> "$RCLONE_TEST_LOG"
        ;;
      *)
        cp "$src" "$dst"
        ;;
    esac
    ;;
  lsjson)
    src="$(map_path "$2")"
    case "$src" in
      */files/sha256/*)
        printf 'start %s\n' "$src" >> "$RCLONE_TEST_LOG"
        sleep 1
        if [ -f "$src" ]; then
          size=$(wc -c < "$src" | tr -d ' \t')
          printf '[{"Path":"%s","Size":%s}]\n' "$(basename "$src")" "$size"
        elif [ -e "$src" ]; then
          printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
        else
          printf '[]\n'
        fi
        printf 'end %s\n' "$src" >> "$RCLONE_TEST_LOG"
        ;;
      *)
        if [ -e "$src" ]; then
          printf '[{"Path":"%s","Size":0}]\n' "$(basename "$src")"
        else
          printf '[]\n'
        fi
        ;;
    esac
    ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  lsd)
    src="$(map_path "$2")"
    if [ -d "$src" ]; then exit 0; else printf 'directory not found: %s\n' "$src" >&2; exit 1; fi
    ;;
  *)
    exit 2
    ;;
esac
`)
}

func assertParallelStarts(t *testing.T, logPath, label string) {
	t.Helper()
	log, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(log)), "\n")
	// Find any two consecutive "start" lines, indicating parallel execution.
	// Non-parallel preamble calls (e.g. preflight ping) may appear first.
	for i := 0; i < len(lines)-1; i++ {
		if strings.HasPrefix(lines[i], "start ") && strings.HasPrefix(lines[i+1], "start ") {
			return
		}
	}
	t.Fatalf("%s did not start in parallel:\n%s", label, log)
}

func mustCopy(t *testing.T, src, dst string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0o644); err != nil {
		t.Fatal(err)
	}
}

func inDir(t *testing.T, dir string, fn func()) {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := os.Chdir(wd); err != nil {
			t.Fatal(err)
		}
	}()
	fn()
}
