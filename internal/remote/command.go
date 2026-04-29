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

	"github.com/vadstup/merk/internal/hash"
)

type rsyncRemote struct{ url string }
type sshRemote struct{ url string }

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

func (r rsyncRemote) remotePath(h hash.Hash) string {
	return r.url + "/objects/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

func (r rsyncRemote) HasObject(ctx context.Context, h hash.Hash) (bool, error) {
	tmp, err := os.CreateTemp("", "merk-remote-check-*")
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

// PushObject uploads to a temporary remote object name before renaming it into
// the final content-addressed path.
func (r rsyncRemote) PushObject(ctx context.Context, h hash.Hash, srcPath string) error {
	if err := hash.VerifyFile(srcPath, h); err != nil {
		return err
	}
	if has, err := r.HasObject(ctx, h); err != nil {
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
		return fmt.Errorf("publish remote object: %w", err)
	}
	has, err := r.HasObject(ctx, h)
	if err != nil {
		return err
	}
	if !has {
		return fmt.Errorf("uploaded remote object failed verification: %s", h)
	}
	return nil
}

// PullObject downloads to a temporary local file and accepts it only after the
// downloaded bytes match the hash encoded in the Git symlink.
func (r rsyncRemote) PullObject(ctx context.Context, h hash.Hash, dstPath string) error {
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
	return os.Rename(tmp, dstPath)
}

func (r sshRemote) HasObject(ctx context.Context, h hash.Hash) (bool, error) {
	return rsyncRemote{url: r.url}.HasObject(ctx, h)
}

func (r sshRemote) PushObject(ctx context.Context, h hash.Hash, srcPath string) error {
	return rsyncRemote{url: r.url}.PushObject(ctx, h, srcPath)
}

func (r sshRemote) PullObject(ctx context.Context, h hash.Hash, dstPath string) error {
	return rsyncRemote{url: r.url}.PullObject(ctx, h, dstPath)
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
	cmdArgs := []string{sshHost(url), "sh", "-c", script, "merk"}
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
