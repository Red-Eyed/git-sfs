package remote

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"git-sfs/internal/errs"
	"git-sfs/internal/fsutil"
	"git-sfs/internal/hash"
)

type rcloneRemote struct {
	url    string
	config string
	debug  io.Writer
}

func NewRclone(url string) Remote {
	return NewRcloneWithOptions(url, Options{})
}

func NewRcloneWithOptions(url string, opts Options) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/"), config: opts.RcloneConfig, debug: opts.Debug}
}

func NewRcloneTarget(host, path string) Remote {
	return NewRcloneTargetWithOptions(host, path, Options{})
}

func NewRcloneTargetWithOptions(host, path string, opts Options) Remote {
	if host == "" {
		return NewRcloneWithOptions(path, opts)
	}
	path = strings.TrimRight(path, "/")
	if strings.HasPrefix(path, "/") || isWindowsAbsPath(path) {
		return rcloneRemote{url: host + ":" + path, config: opts.RcloneConfig, debug: opts.Debug}
	}
	return rcloneRemote{url: host + ":" + strings.TrimLeft(path, "/"), config: opts.RcloneConfig, debug: opts.Debug}
}

func isWindowsAbsPath(path string) bool {
	return len(path) >= 3 && path[1] == ':' && path[2] == '/'
}

func (r rcloneRemote) remotePath(h hash.Hash) string {
	return r.url + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

func (r rcloneRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	tmp, err := os.CreateTemp("", "git-sfs-rclone-has-*")
	if err != nil {
		return false, err
	}
	name := tmp.Name()
	tmp.Close()
	os.Remove(name)
	defer os.Remove(name)
	if err := r.run(ctx, "copyto", r.remotePath(h), name); err != nil {
		if ctx.Err() != nil {
			return false, ctx.Err()
		}
		return false, nil
	}
	return true, nil
}

func (r rcloneRemote) CheckFile(ctx context.Context, h hash.Hash) (bool, error) {
	tmp, err := os.CreateTemp("", "git-sfs-rclone-check-*")
	if err != nil {
		return false, err
	}
	name := tmp.Name()
	tmp.Close()
	os.Remove(name)
	defer os.Remove(name)
	if err := r.run(ctx, "copyto", r.remotePath(h), name); err != nil {
		if ctx.Err() != nil {
			return false, ctx.Err()
		}
		return false, nil
	}
	if err := hash.VerifyFile(name, h); err != nil {
		return false, errors.Join(errs.ErrCorruptRemoteFile, err)
	}
	return true, nil
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
	if err := r.run(ctx, "copyto", srcPath, tmp); err != nil {
		return err
	}
	if err := r.run(ctx, "moveto", tmp, dst); err != nil {
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
	if err := r.run(ctx, "copyto", r.remotePath(h), tmp); err != nil {
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

func (r rcloneRemote) run(ctx context.Context, args ...string) error {
	_, err := r.runOutput(ctx, args...)
	return err
}

func (r rcloneRemote) runOutput(ctx context.Context, args ...string) (string, error) {
	if r.config != "" {
		args = append([]string{"--config", r.config}, args...)
	}
	return runOutput(ctx, r.debug, "rclone", args...)
}

func run(ctx context.Context, debug io.Writer, name string, args ...string) error {
	_, err := runOutput(ctx, debug, name, args...)
	return err
}

func runOutput(ctx context.Context, debug io.Writer, name string, args ...string) (string, error) {
	if debug != nil {
		fmt.Fprintln(debug, "run:", shellQuote(append([]string{name}, args...)))
	}
	cmd := exec.CommandContext(ctx, name, args...)
	out, err := cmd.CombinedOutput()
	if err == nil {
		return string(out), nil
	}
	if ctx.Err() != nil {
		return "", ctx.Err()
	}
	if len(out) == 0 {
		return "", err
	}
	return "", fmt.Errorf("%w: %s", err, strings.TrimSpace(string(out)))
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
