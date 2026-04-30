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

func TestNewCommandRemotesUseFilesystemForLocalPaths(t *testing.T) {
	if _, ok := NewRsync(t.TempDir()).(filesystemRemote); !ok {
		t.Fatal("local rsync path should use filesystem remote")
	}
	if _, ok := NewSSH(t.TempDir()).(filesystemRemote); !ok {
		t.Fatal("local ssh path should use filesystem remote")
	}
	if _, ok := NewRsync(`C:\data`).(filesystemRemote); !ok {
		t.Fatal("windows rsync drive path should use filesystem remote")
	}
	if _, ok := NewSSH("D:/data").(filesystemRemote); !ok {
		t.Fatal("windows ssh drive path should use filesystem remote")
	}
	if _, ok := NewRsync("host:/data").(rsyncRemote); !ok {
		t.Fatal("host rsync path should use rsync remote")
	}
	if _, ok := NewRsync("host:D:/data").(rsyncRemote); !ok {
		t.Fatal("host rsync windows drive path should use rsync remote")
	}
	if _, ok := NewSSH("host:/data").(sshRemote); !ok {
		t.Fatal("host ssh path should use ssh remote")
	}
	if _, ok := NewSSH("host:D:/data").(sshRemote); !ok {
		t.Fatal("host ssh windows drive path should use ssh remote")
	}
	if got := NewRsyncTarget("host:2222", "D:/data").(rsyncRemote); got.remoteRoot() != "host:D:/data" {
		t.Fatalf("host/path rsync remote should keep port out of rsync path: %#v", got)
	}
	if got := NewSSHTarget("storage", "D:/data").(sshRemote); got.host != "storage" || got.path != "D:/data" {
		t.Fatalf("bad ssh host/path remote: %#v", got)
	}
	if got := NewRcloneTarget("remote-name", "D:/data").(rcloneRemote); got.url != "remote-name:D:/data" {
		t.Fatalf("bad rclone host/path remote: %#v", got)
	}
	if got := NewRsyncTargetWithOptions("host", "D:/data", Options{Shell: "cmd"}).(rsyncRemote); got.shell != "cmd" {
		t.Fatalf("bad rsync shell: %#v", got)
	}
}

func TestCommandRemoteHelpers(t *testing.T) {
	if sshHost("user@host:/data") != "user@host" {
		t.Fatal("bad ssh host")
	}
	if remoteLocalPath("user@host:/data") != "/data" {
		t.Fatal("bad remote path")
	}
	if !isWindowsDrivePath(`C:\data`) || !isWindowsDrivePath("D:/data") {
		t.Fatal("windows drive path was not detected")
	}
	if isCommandRemoteURL("C:/data") {
		t.Fatal("windows drive path should not be a command remote")
	}
	if !isCommandRemoteURL("user@host:C:/data") {
		t.Fatal("remote windows drive path should be a command remote")
	}
	if sshHost("user@host:C:/data") != "user@host" {
		t.Fatal("bad windows remote ssh host")
	}
	if remoteLocalPath("user@host:C:/data") != "C:/data" {
		t.Fatal("bad windows remote local path")
	}
	host, port := splitHostPort("user@host:2222")
	if host != "user@host" || port != "2222" {
		t.Fatalf("bad host port split: host=%q port=%q", host, port)
	}
	host, port = splitHostPort("storage")
	if host != "storage" || port != "" {
		t.Fatalf("bad ssh config host split: host=%q port=%q", host, port)
	}
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if got := (rsyncRemote{url: "host:/root"}).remotePath(h); got != "host:/root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad remote path %q", got)
	}
	if got := (rsyncRemote{url: "host:D:/root"}).remotePath(h); got != "host:D:/root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad windows rsync remote path %q", got)
	}
	if got := (rcloneRemote{url: "remote:root"}).remotePath(h); got != "remote:root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad rclone remote path %q", got)
	}
	if got := (rcloneRemote{url: "remote:D:/root"}).remotePath(h); got != "remote:D:/root/files/sha256/aa/"+h.String() {
		t.Fatalf("bad windows rclone remote path %q", got)
	}
}

func TestNewLocalSSHRemoteUsesFilesystem(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	r := NewSSH(filepath.Join(dir, "remote"))
	if err := r.PushFile(ctx, h, src); err != nil {
		t.Fatal(err)
	}
	has, err := r.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if !has {
		t.Fatal("file missing")
	}
	dst := filepath.Join(dir, "dst")
	if err := r.PullFile(ctx, h, dst); err != nil {
		t.Fatal(err)
	}
	if err := hash.VerifyFile(dst, h); err != nil {
		t.Fatal(err)
	}
}

func TestRsyncRemoteWithFakeCommands(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell scripts are not used on windows")
	}
	ctx := context.Background()
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	writeTool(t, filepath.Join(bin, "rsync"), `set -eu
while [ "${1:-}" = "--partial" ] || [ "${1:-}" = "--quiet" ]; do
  shift
done
src="$1"
dst="$2"
case "$src" in *:*) src="${src#*:}" ;; esac
case "$dst" in *:*) dst="${dst#*:}" ;; esac
mkdir -p "$(dirname "$dst")"
cp "$src" "$dst"
`)
	writeTool(t, filepath.Join(bin, "ssh"), `set -eu
shift
if [ "$1" != "sh" ] || [ "$2" != "-c" ]; then
  exit 1
fi
script="$3"
shift 4
sh -c "$script" git-sfs "$@"
`)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))

	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	remoteRoot := filepath.Join(dir, "remote")
	r := rsyncRemote{url: "host:" + remoteRoot}
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

	s := sshRemote{url: "host:" + remoteRoot}
	has, err = s.HasFile(ctx, h)
	if err != nil {
		t.Fatal(err)
	}
	if !has {
		t.Fatal("ssh remote should delegate has")
	}
	dst2 := filepath.Join(dir, "dst2")
	if err := s.PullFile(ctx, h, dst2); err != nil {
		t.Fatal(err)
	}
	if err := hash.VerifyFile(dst2, h); err != nil {
		t.Fatal(err)
	}
	src2 := filepath.Join(dir, "src2")
	if err := os.WriteFile(src2, []byte("other"), 0o644); err != nil {
		t.Fatal(err)
	}
	h2, err := hash.File(src2)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.PushFile(ctx, h2, src2); err != nil {
		t.Fatal(err)
	}
}

func TestRsyncRemoteRejectsBadSource(t *testing.T) {
	h := hash.Hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	if err := (rsyncRemote{url: "host:/remote"}).PushFile(context.Background(), h, filepath.Join(t.TempDir(), "missing")); err == nil {
		t.Fatal("expected missing source error")
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
cmd="$1"
src="$2"
dst="$3"
map_path() {
  case "$1" in
    testremote:*) printf '%s/%s\n' "$RCLONE_TEST_ROOT" "${1#testremote:}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}
src="$(map_path "$src")"
dst="$(map_path "$dst")"
case "$cmd" in
  copyto)
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst"
    ;;
  moveto)
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

	src := filepath.Join(dir, "src")
	if err := os.WriteFile(src, []byte("payload"), 0o644); err != nil {
		t.Fatal(err)
	}
	h, err := hash.File(src)
	if err != nil {
		t.Fatal(err)
	}
	r := rcloneRemote{url: "testremote:dataset"}
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

func writeTool(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
}
