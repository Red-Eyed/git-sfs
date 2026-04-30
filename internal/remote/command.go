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

type rsyncRemote struct{ url string }
type sshRemote struct{ url string }
type rcloneRemote struct{ url string }

// NewRsync accepts local filesystem paths too, which keeps tests and single-box
// workflows from needing an ssh server.
func NewRsync(url string) Remote {
	url = strings.TrimRight(url, "/")
	if !strings.Contains(url, ":") {
		return NewFilesystem(url)
	}
	return rsyncRemote{url: url}
}

func NewSSH(url string) Remote {
	url = strings.TrimRight(url, "/")
	if !strings.Contains(url, ":") {
		return NewFilesystem(url)
	}
	return sshRemote{url: url}
}

func NewRclone(url string) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/")}
}

func (r rsyncRemote) remotePath(h hash.Hash) string {
	return r.url + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

func (r rcloneRemote) remotePath(h hash.Hash) string {
	return r.url + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
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
	if err := run(ctx, "rsync", "--quiet", r.remotePath(h), name); err != nil {
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
	if err := sshSh(ctx, r.url, "mkdir -p \"$1\"", remoteLocalPath(dir)); err != nil {
		return fmt.Errorf("create remote dir: %w", err)
	}
	tmp := dst + ".tmp-" + strconv.Itoa(os.Getpid()) + "-" + strconv.FormatInt(time.Now().UnixNano(), 10)
	if err := run(ctx, "rsync", "--partial", srcPath, tmp); err != nil {
		return err
	}
	if err := sshSh(ctx, r.url, "mv \"$1\" \"$2\"", remoteLocalPath(tmp), remoteLocalPath(dst)); err != nil {
		return fmt.Errorf("publish remote file: %w", err)
	}
	if err := sshSh(ctx, r.url, "chmod a-w \"$1\"", remoteLocalPath(dst)); err != nil {
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
	if err := run(ctx, "rsync", "--partial", r.remotePath(h), tmp); err != nil {
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
	return rsyncRemote{url: r.url}.HasFile(ctx, h)
}

func (r sshRemote) PushFile(ctx context.Context, h hash.Hash, srcPath string) error {
	return rsyncRemote{url: r.url}.PushFile(ctx, h, srcPath)
}

func (r sshRemote) PullFile(ctx context.Context, h hash.Hash, dstPath string) error {
	return rsyncRemote{url: r.url}.PullFile(ctx, h, dstPath)
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
	if i := strings.Index(url, ":"); i >= 0 {
		return url[:i]
	}
	return url
}

func remoteLocalPath(url string) string {
	if i := strings.Index(url, ":"); i >= 0 {
		return url[i+1:]
	}
	return url
}

func sshSh(ctx context.Context, url, script string, args ...string) error {
	cmdArgs := []string{sshHost(url), "sh", "-c", script, "git-sfs"}
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
