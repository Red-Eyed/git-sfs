package remote

import (
	"context"
	"fmt"
	"io"
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
	url   string
	host  string
	path  string
	shell string
	debug io.Writer
}
type sshRemote struct {
	url   string
	host  string
	path  string
	shell string
	debug io.Writer
}
type rcloneRemote struct {
	url   string
	debug io.Writer
}

// NewRsync accepts local filesystem paths too, which keeps tests and single-box
// workflows from needing an ssh server.
func NewRsync(url string) Remote {
	return NewRsyncWithOptions(url, Options{})
}

func NewRsyncWithOptions(url string, opts Options) Remote {
	url = strings.TrimRight(url, "/")
	if !isCommandRemoteURL(url) {
		return NewFilesystem(url)
	}
	return rsyncRemote{url: url, shell: commandShell(opts), debug: opts.Debug}
}

func NewRsyncTarget(host, path string) Remote {
	return NewRsyncTargetWithOptions(host, path, Options{})
}

func NewRsyncTargetWithOptions(host, path string, opts Options) Remote {
	if host == "" {
		return NewFilesystem(path)
	}
	return rsyncRemote{host: host, path: strings.TrimRight(path, "/"), shell: commandShell(opts), debug: opts.Debug}
}

func NewSSH(url string) Remote {
	return NewSSHWithOptions(url, Options{})
}

func NewSSHWithOptions(url string, opts Options) Remote {
	url = strings.TrimRight(url, "/")
	if !isCommandRemoteURL(url) {
		return NewFilesystem(url)
	}
	return sshRemote{url: url, shell: commandShell(opts), debug: opts.Debug}
}

func NewSSHTarget(host, path string) Remote {
	return NewSSHTargetWithOptions(host, path, Options{})
}

func NewSSHTargetWithOptions(host, path string, opts Options) Remote {
	if host == "" {
		return NewFilesystem(path)
	}
	return sshRemote{host: host, path: strings.TrimRight(path, "/"), shell: commandShell(opts), debug: opts.Debug}
}

func NewRclone(url string) Remote {
	return NewRcloneWithOptions(url, Options{})
}

func NewRcloneWithOptions(url string, opts Options) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/"), debug: opts.Debug}
}

func NewRcloneTarget(host, path string) Remote {
	return NewRcloneTargetWithOptions(host, path, Options{})
}

func NewRcloneTargetWithOptions(host, path string, opts Options) Remote {
	if host == "" {
		return NewRcloneWithOptions(path, opts)
	}
	return rcloneRemote{url: host + ":" + strings.TrimLeft(strings.TrimRight(path, "/"), "/"), debug: opts.Debug}
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
	return run(ctx, r.debug, "rsync", cmdArgs...)
}

func (r rsyncRemote) remoteMkdir(ctx context.Context, path string) error {
	if r.shell == "cmd" {
		return sshCmd(ctx, r.debug, r.hostForCommand(), "cmd", "/C", "if", "not", "exist", path, "mkdir", path)
	}
	return sshSh(ctx, r.debug, r.hostForCommand(), "mkdir -p \"$1\"", path)
}

func (r rsyncRemote) remoteMove(ctx context.Context, src, dst string) error {
	if r.shell == "cmd" {
		return sshCmd(ctx, r.debug, r.hostForCommand(), "cmd", "/C", "move", "/Y", src, dst)
	}
	return sshSh(ctx, r.debug, r.hostForCommand(), "mv \"$1\" \"$2\"", src, dst)
}

func (r rsyncRemote) remoteProtect(ctx context.Context, path string) error {
	if r.shell == "cmd" {
		return sshCmd(ctx, r.debug, r.hostForCommand(), "cmd", "/C", "attrib", "+R", path)
	}
	return sshSh(ctx, r.debug, r.hostForCommand(), "chmod a-w \"$1\"", path)
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
	if err := r.remoteMkdir(ctx, remoteLocalPath(dir)); err != nil {
		return fmt.Errorf("create remote dir: %w", err)
	}
	tmp := dst + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	if err := r.rsync(ctx, "--partial", srcPath, tmp); err != nil {
		return err
	}
	if err := r.remoteMove(ctx, remoteLocalPath(tmp), remoteLocalPath(dst)); err != nil {
		return fmt.Errorf("publish remote file: %w", err)
	}
	if err := r.remoteProtect(ctx, remoteLocalPath(dst)); err != nil {
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
	return rsyncRemote{url: r.url, host: r.host, path: r.path, shell: r.shell, debug: r.debug}.HasFile(ctx, h)
}

func (r sshRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	return rsyncRemote{url: r.url, host: r.host, path: r.path, shell: r.shell, debug: r.debug}.PushFile(ctx, h, srcPath)
}

func (r sshRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	return rsyncRemote{url: r.url, host: r.host, path: r.path, shell: r.shell, debug: r.debug}.PullFile(ctx, h, dstPath)
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
	if err := run(ctx, r.debug, "rclone", "copyto", r.remotePath(h), name); err != nil {
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
	if err := run(ctx, r.debug, "rclone", "copyto", srcPath, tmp); err != nil {
		return err
	}
	if err := run(ctx, r.debug, "rclone", "moveto", tmp, dst); err != nil {
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
	if err := run(ctx, r.debug, "rclone", "copyto", r.remotePath(h), tmp); err != nil {
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

func commandShell(opts Options) string {
	if opts.Shell == "cmd" {
		return "cmd"
	}
	return "sh"
}

func sshSh(ctx context.Context, debug io.Writer, host, script string, args ...string) error {
	cmdArgs := sshArgs(host)
	cmdArgs = append(cmdArgs, "sh", "-c", script, "git-sfs")
	cmdArgs = append(cmdArgs, args...)
	return run(ctx, debug, "ssh", cmdArgs...)
}

func sshCmd(ctx context.Context, debug io.Writer, host string, args ...string) error {
	cmdArgs := sshArgs(host)
	cmdArgs = append(cmdArgs, args...)
	return run(ctx, debug, "ssh", cmdArgs...)
}

func sshArgs(host string) []string {
	sshHost, port := splitHostPort(host)
	cmdArgs := []string{}
	if port != "" {
		cmdArgs = append(cmdArgs, "-p", port)
	}
	return append(cmdArgs, sshHost)
}

func run(ctx context.Context, debug io.Writer, name string, args ...string) error {
	if debug != nil {
		fmt.Fprintln(debug, "run:", shellQuote(append([]string{name}, args...)))
	}
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

func shellQuote(args []string) string {
	parts := make([]string, 0, len(args))
	for _, arg := range args {
		if arg == "" || strings.ContainsAny(arg, " \t\n\"'\\") {
			parts = append(parts, strconv.Quote(arg))
			continue
		}
		parts = append(parts, arg)
	}
	return strings.Join(parts, " ")
}
