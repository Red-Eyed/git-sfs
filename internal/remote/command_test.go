package remote

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"git-sfs/internal/hash"
)

func TestRcloneTarget(t *testing.T) {
	if got := NewRcloneTarget("local", "/tmp/data").(rcloneRemote); got.url != "local:/tmp/data" {
		t.Fatalf("bad absolute unix host/path remote: %#v", got)
	}
	if got := NewRcloneTarget("remote-name", "dataset/root").(rcloneRemote); got.url != "remote-name:dataset/root" {
		t.Fatalf("bad relative host/path remote: %#v", got)
	}
	if got := NewRcloneTarget("remote-name", "/dataset/root/").(rcloneRemote); got.url != "remote-name:/dataset/root" {
		t.Fatalf("bad absolute host/path remote with trailing slash: %#v", got)
	}
	if got := NewRcloneTarget("remote-name", "D:/data").(rcloneRemote); got.url != "remote-name:D:/data" {
		t.Fatalf("bad rclone host/path remote: %#v", got)
	}
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if got := (rcloneRemote{url: "remote:root"}).remotePath(h); got != "remote:root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad rclone remote path %q", got)
	}
	if got := (rcloneRemote{url: "remote:D:/root"}).remotePath(h); got != "remote:D:/root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad windows rclone remote path %q", got)
	}
}

func TestRcloneRemoteWithFakeCommand(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	ctx := context.Background()
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "rclone"), `set -eu
if [ "${1:-}" = "--config" ]; then
  printf '%s\n' "$2" >> "$RCLONE_TEST_CONFIG_LOG"
  shift 2
fi
cmd="$1"
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
    cp "$src" "$dst"
    ;;
  lsjson)
    src="$(map_path "$2")"
    if [ -e "$src" ]; then
      printf '[{"Path":"%s"}]\n' "$(basename "$src")"
    else
      printf '[]\n'
    fi
    ;;
  moveto)
    src="$(map_path "$2")"
    dst="$(map_path "$3")"
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"
    ;;
  *)
    exit 2
    ;;
esac
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("RCLONE_TEST_ROOT", filepath.Join(dir, "remote"))
	configLog := filepath.Join(dir, "config-log")
	t.Setenv("RCLONE_TEST_CONFIG_LOG", configLog)

	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	r := rcloneRemote{url: "testremote:dataset", config: filepath.Join(dir, ".git-sfs", "rclone.conf")}
	has, err := r.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if has {
		t.Fatal("remote should start empty")
	}
	if err := r.PushFile(ctx, h, src); err != nil {
		t.Fatal(err)
	}
	has, err = r.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if !has {
		t.Fatal("remote should have file")
	}
	dst := filepath.Join(dir, "dst")
	if err := r.PullFile(ctx, h, dst); err != nil {
		t.Fatal(err)
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		t.Fatal(err)
	}
	log, err := os.ReadFile(configLog)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(log), filepath.Join(dir, ".git-sfs", "rclone.conf")) {
		t.Fatalf("rclone config was not passed:\n%s", log)
	}
}

func TestRunIncludesCommandOutput(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "bad"), `echo nope >&2
exit 9
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	err := run(context.Background(), nil, "bad")
	if err == nil || !strings.Contains(err.Error(), "nope") {
		t.Fatalf("missing command output: %v", err)
	}
}

func TestRunWritesDebugCommand(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "ok"), `exit 0
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	var debug bytes.Buffer
	if err := run(context.Background(), &debug, "ok", "arg with space"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(debug.String(), "run: ok") || !strings.Contains(debug.String(), `"arg with space"`) {
		t.Fatalf("missing debug command: %q", debug.String())
	}
}

func TestRunWithRetrySucceedsOnSecondAttempt(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	counter := filepath.Join(dir, "counter")
	// Script fails on first call, succeeds on second.
	writeTool(t, filepath.Join(bin, "flaky"), `
count=0
if [ -f "`+counter+`" ]; then
  count=$(cat "`+counter+`")
fi
count=$((count + 1))
printf '%d' "$count" > "`+counter+`"
if [ "$count" -lt 2 ]; then
  echo "transient error" >&2
  exit 1
fi
echo "ok"
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	out, err := runWithRetry(context.Background(), nil, 3, "flaky")
	if err != nil {
		t.Fatalf("expected success on second attempt, got: %v", err)
	}
	if !strings.Contains(out, "ok") {
		t.Fatalf("unexpected output: %q", out)
	}
}

func TestRunWithRetryRespectsContextCancellation(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "always-fail"), `exit 1
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately
	_, err := runWithRetry(ctx, nil, 3, "always-fail")
	if err == nil {
		t.Fatal("expected error for cancelled context")
	}
	if !strings.Contains(err.Error(), "context") {
		t.Fatalf("expected context error, got: %v", err)
	}
}

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}
