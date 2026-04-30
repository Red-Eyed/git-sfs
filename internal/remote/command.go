package remote

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"git-sfs/internal/fsutil"
	"git-sfs/internal/hash"
)

type rsyncRemote struct {
	url  string
	host string
	path string
}
type sshRemote struct {
	url  string
	host string
	path string
}
type rcloneRemote struct{ url string }

// NewRsync accepts local filesystem paths too, which keeps tests and single-box
// workflows from needing an ssh server.
func NewRsync(url string) Remote {
	url = strings.TrimRight(url, "/")
	if !isCommandRemoteURL(url) {
		return NewFilesystem(url)
	}
	return rsyncRemote{url: url}
}

func NewRsyncTarget(host, path string) Remote {
	if host == "" {
		return NewFilesystem(path)
	}
	return rsyncRemote{host: host, path: strings.TrimRight(path, "/")}
}

func NewSSH(url string) Remote {
	url = strings.TrimRight(url, "/")
	if !isCommandRemoteURL(url) {
		return NewFilesystem(url)
	}
	return sshRemote{url: url}
}

func NewSSHTarget(host, path string) Remote {
	if host == "" {
		return NewFilesystem(path)
	}
	return sshRemote{host: host, path: strings.TrimRight(path, "/")}
}

func NewRclone(url string) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/")}
}

func NewRcloneTarget(host, path string) Remote {
	if host == "" {
		return NewRclone(path)
	}
	return rcloneRemote{url: host + ":" + strings.TrimLeft(strings.TrimRight(path, "/"), "/")}
}

func (r rsyncRemote) remotePath(h hash.Hash) string {
	return r.remoteRoot() + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

func (r rcloneRemote) remotePath(h hash.Hash) string {
	return r.url + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

func (r rsyncRemote) remoteRoot() string {
	if r.host != "" {
		host, _ := r.hostPort()
		return host + ":" + r.path
	}
	return r.url
}

func (r rsyncRemote) hostPort() (string, string) {
	if r.host != "" {
		return splitHostPort(r.host)
	}
	return splitHostPort(sshHost(r.url))
}

func (r rsyncRemote) hostForCommand() string {
	host, _ := r.hostPort()
	return host
}

func (r rsyncRemote) rsync(ctx context.Context, args ...string) error {
	host, port := r.hostPort()
	cmdArgs := []string{}
	if port != "" && host != "" {
		cmdArgs = append(cmdArgs, "-e", "ssh -p "+port)
	}
	cmdArgs = append(cmdArgs, args...)
	return run(ctx, "rsync", cmdArgs...)
}

func (r rsyncRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	tmp, err := os.CreateTemp("", "git-sfs-remote-check-*")
	if err != nil {
		return false, err
	}
	name := tmp.Name()
	tmp.Close()
	os.Remove(name)
	defer os.Remove(name)
	if err := r.rsync(ctx, "--quiet", r.remotePath(h), name); err != nil {
		if ctx.Err() != nil {
			return false, ctx.Err()
		}
		return false, nil
	}
	return hash.VerifyFile(name, h) == nil, nil
}

// PushFile uploads to a temporary remote file name before renaming it into
// the final content-addressed path.
func (r rsyncRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	if err := hash.VerifyFile(srcPath, h); err != nil {
		return err
	}
	if has, err := r.HasFile(ctx, h); err != nil {
		return err
	} else if has {
		return nil
	}
	dst := r.remotePath(h)
	dir := dst[:strings.LastIndex(dst, "/")]
	if err := sshSh(ctx, r.hostForCommand(), "mkdir -p \"$1\"", remoteLocalPath(dir)); err != nil {
		return fmt.Errorf("create remote dir: %w", err)
	}
	tmp := dst + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	if err := r.rsync(ctx, "--partial", srcPath, tmp); err != nil {
		return err
	}
	if err := sshSh(ctx, r.hostForCommand(), "mv \"$1\" \"$2\"", remoteLocalPath(tmp), remoteLocalPath(dst)); err != nil {
		return fmt.Errorf("publish remote file: %w", err)
	}
	if err := sshSh(ctx, r.hostForCommand(), "chmod a-w \"$1\"", remoteLocalPath(dst)); err != nil {
		return fmt.Errorf("protect remote file: %w", err)
	}
	has, err := r.HasFile(ctx, h)
	if err != nil {
		return err
	}
	if !has {
		return fmt.Errorf("uploaded remote file failed verification: %s", h)
	}
	return nil
}

// PullFile downloads to a temporary local file and accepts it only after the
// downloaded bytes match the hash encoded in the Git symlink.
func (r rsyncRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	if err := os.MkdirAll(filepath.Dir(dstPath), 0o755); err != nil {
		return err
	}
	tmp := dstPath + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	defer os.Remove(tmp)
	if err := r.rsync(ctx, "--partial", r.remotePath(h), tmp); err != nil {
		return err
	}
	if err := hash.VerifyFile(tmp, h); err != nil {
		return err
	}
	if err := os.Chmod(tmp, fsutil.ReadOnlyMode(0o644)); err != nil {
		return err
	}
	if err := os.Rename(tmp, dstPath); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(dstPath)
}

func (r sshRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	return rsyncRemote{url: r.url, host: r.host, path: r.path}.HasFile(ctx, h)
}

func (r sshRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	return rsyncRemote{url: r.url, host: r.host, path: r.path}.PushFile(ctx, h, srcPath)
}

func (r sshRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	return rsyncRemote{url: r.url, host: r.host, path: r.path}.PullFile(ctx, h, dstPath)
}

func (r rcloneRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	tmp, err := os.CreateTemp("", "git-sfs-rclone-check-*")
	if err != nil {
		return false, err
	}
	name := tmp.Name()
	tmp.Close()
	os.Remove(name)
	defer os.Remove(name)
	if err := run(ctx, "rclone", "copyto", r.remotePath(h), name); err != nil {
		if ctx.Err() != nil {
			return false, ctx.Err()
		}
		return false, nil
	}
	return hash.VerifyFile(name, h) == nil, nil
}

func (r rcloneRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	if err := hash.VerifyFile(srcPath, h); err != nil {
		return err
	}
	if has, err := r.HasFile(ctx, h); err != nil {
		return err
	} else if has {
		return nil
	}
	dst := r.remotePath(h)
	tmp := dst + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	if err := run(ctx, "rclone", "copyto", srcPath, tmp); err != nil {
		return err
	}
	if err := run(ctx, "rclone", "moveto", tmp, dst); err != nil {
		return fmt.Errorf("publish remote file: %w", err)
	}
	has, err := r.HasFile(ctx, h)
	if err != nil {
		return err
	}
	if !has {
		return fmt.Errorf("uploaded remote file failed verification: %s", h)
	}
	return nil
}

func (r rcloneRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	if err := os.MkdirAll(filepath.Dir(dstPath), 0o755); err != nil {
		return err
	}
	tmp := dstPath + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	defer os.Remove(tmp)
	if err := run(ctx, "rclone", "copyto", r.remotePath(h), tmp); err != nil {
		return err
	}
	if err := hash.VerifyFile(tmp, h); err != nil {
		return err
	}
	if err := os.Chmod(tmp, fsutil.ReadOnlyMode(0o644)); err != nil {
		return err
	}
	if err := os.Rename(tmp, dstPath); err != nil {
		return err
	}
	return fsutil.MakeReadOnly(dstPath)
}

func sshHost(url string) string {
	if i := remoteSeparator(url); i >= 0 {
		return url[:i]
	}
	return url
}

func remoteLocalPath(url string) string {
	if i := remoteSeparator(url); i >= 0 {
		return url[i+1:]
	}
	return url
}

func splitHostPort(host string) (string, string) {
	if isWindowsDrivePath(host) {
		return host, ""
	}
	i := strings.LastIndex(host, ":")
	if i <= 0 || i == len(host)-1 {
		return host, ""
	}
	port := host[i+1:]
	for _, c := range port {
		if c < '0' || c > '9' {
			return host, ""
		}
	}
	return host[:i], port
}

func isCommandRemoteURL(url string) bool {
	return remoteSeparator(url) >= 0
}

func remoteSeparator(url string) int {
	if isWindowsDrivePath(url) {
		return -1
	}
	return strings.Index(url, ":")
}

func isWindowsDrivePath(path string) bool {
	if len(path) < 2 || path[1] != ':' {
		return false
	}
	c := path[0]
	return ('A' <= c && c <= 'Z') || ('a' <= c && c <= 'z')
}

func sshSh(ctx context.Context, host, script string, args ...string) error {
	sshHost, port := splitHostPort(host)
	cmdArgs := []string{}
	if port != "" {
		cmdArgs = append(cmdArgs, "-p", port)
	}
	cmdArgs = append(cmdArgs, sshHost, "sh", "-c", script, "git-sfs")
	cmdArgs = append(cmdArgs, args...)
	return run(ctx, "ssh", cmdArgs...)
}

func run(ctx context.Context, name string, args ...string) error {
	cmd := exec.CommandContext(ctx, name, args...)
	out, err := cmd.CombinedOutput()
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if len(out) == 0 {
		return err
	}
	return fmt.Errorf("%w: %s", err, strings.TrimSpace(string(out)))
}
