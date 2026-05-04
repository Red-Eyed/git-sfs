package remote

import (
	"bytes"
	"context"
	"encoding/json"
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
	url      string
	config   string
	debug    io.Writer
	retryMax int
}

func NewRclone(url string) Remote {
	return NewRcloneWithOptions(url, Options{})
}

func NewRcloneWithOptions(url string, opts Options) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/"), config: opts.RcloneConfig, debug: opts.Debug, retryMax: opts.RetryMax}
}

func NewRcloneTarget(remote, path string) Remote {
	return NewRcloneTargetWithOptions(remote, path, Options{})
}

func NewRcloneTargetWithOptions(remote, path string, opts Options) Remote {
	if remote == "" {
		return NewRcloneWithOptions(path, opts)
	}
	path = strings.TrimRight(path, "/")
	if strings.HasPrefix(path, "/") || isWindowsAbsPath(path) {
		return rcloneRemote{url: remote + ":" + path, config: opts.RcloneConfig, debug: opts.Debug, retryMax: opts.RetryMax}
	}
	return rcloneRemote{url: remote + ":" + strings.TrimLeft(path, "/"), config: opts.RcloneConfig, debug: opts.Debug, retryMax: opts.RetryMax}
}

func isWindowsAbsPath(path string) bool {
	return len(path) >= 3 && path[1] == ':' && path[2] == '/'
}

func (r rcloneRemote) remotePath(h hash.Hash) string {
	return r.url + "/files/" + hash.Algorithm + "/" + h.Prefix() + "/" + h.String()
}

// CheckRcloneOnPath returns a non-nil error if the rclone binary is not on PATH.
func CheckRcloneOnPath() error {
	if _, err := exec.LookPath("rclone"); err != nil {
		return fmt.Errorf("rclone not found on PATH: %w", err)
	}
	return nil
}

// RequireExists checks that the remote root exists. Unlike Ping, a missing
// path is an error — use this before push to prevent accidental creation of
// files at a wrong path.
func (r rcloneRemote) RequireExists(ctx context.Context) error {
	out, err := r.runOutput(ctx, "lsjson", r.url)
	if err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		msg := strings.ToLower(err.Error())
		if strings.Contains(msg, "not found") || strings.Contains(msg, "no such file") {
			return fmt.Errorf("remote path does not exist: %s (create it before pushing)", r.url)
		}
		return fmt.Errorf("remote unreachable (%s): %w", r.url, err)
	}
	// An empty listing is fine — the remote directory exists but has no files yet.
	_ = out
	return nil
}

func (r rcloneRemote) HasFile(ctx context.Context, h hash.Hash) (bool, error) {
	out, err := r.runOutput(ctx, "lsjson", r.remotePath(h))
	if err != nil {
		if ctx.Err() != nil {
			return false, ctx.Err()
		}
		return false, nil
	}
	return parseLSJSONExists(out)
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
	return os.Rename(tmp, dstPath)
}

func (r rcloneRemote) run(ctx context.Context, args ...string) error {
	_, err := r.runOutput(ctx, args...)
	return err
}

func (r rcloneRemote) runOutput(ctx context.Context, args ...string) (string, error) {
	if r.config != "" {
		args = append([]string{"--config", r.config}, args...)
	}
	return runWithRetry(ctx, r.debug, r.retryMax, "rclone", args...)
}

// runWithRetry calls runOutput up to retryMax times with exponential backoff.
// A zero or negative retryMax uses the default of 3.
func runWithRetry(ctx context.Context, debug io.Writer, retryMax int, name string, args ...string) (string, error) {
	if retryMax <= 0 {
		retryMax = 3
	}
	backoff := time.Second
	var lastErr error
	for attempt := 1; attempt <= retryMax; attempt++ {
		out, err := runOutput(ctx, debug, name, args...)
		if err == nil {
			return out, nil
		}
		if ctx.Err() != nil {
			return "", ctx.Err()
		}
		lastErr = err
		if attempt < retryMax {
			if debug != nil {
				fmt.Fprintf(debug, "retry %d/%d after %s: %v\n", attempt, retryMax, backoff, err)
			}
			select {
			case <-ctx.Done():
				return "", ctx.Err()
			case <-time.After(backoff):
			}
			backoff *= 2
		}
	}
	return "", lastErr
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
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err == nil {
		return stdout.String(), nil
	}
	if ctx.Err() != nil {
		return "", ctx.Err()
	}
	msg := strings.TrimSpace(stderr.String())
	if msg == "" {
		return "", err
	}
	return "", fmt.Errorf("%w: %s", err, msg)
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

// DetectRcloneVersion runs "rclone version" and extracts the version string (e.g. "1.67.0").
// rcloneConfig is passed via --config if non-empty.
func DetectRcloneVersion(ctx context.Context, rcloneConfig string) (string, error) {
	args := []string{"version"}
	if rcloneConfig != "" {
		args = append([]string{"--config", rcloneConfig}, args...)
	}
	out, err := runOutput(ctx, nil, "rclone", args...)
	if err != nil {
		return "", fmt.Errorf("detect rclone version: %w", err)
	}
	for _, line := range strings.SplitAfter(out, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "rclone v") {
			// line looks like "rclone v1.67.0"
			ver := strings.TrimPrefix(line, "rclone v")
			ver = strings.Fields(ver)[0] // strip any trailing text
			return ver, nil
		}
	}
	return "", fmt.Errorf("could not parse rclone version from output: %q", out)
}

func parseLSJSONExists(out string) (bool, error) {
	trimmed := bytes.TrimSpace([]byte(out))
	if len(trimmed) == 0 || bytes.Equal(trimmed, []byte("[]")) {
		return false, nil
	}
	var items []json.RawMessage
	if err := json.Unmarshal(trimmed, &items); err != nil {
		return false, fmt.Errorf("parse rclone lsjson output: %w", err)
	}
	return len(items) > 0, nil
}

func parseLSJSONSize(out string) (int64, error) {
	trimmed := bytes.TrimSpace([]byte(out))
	if len(trimmed) == 0 || bytes.Equal(trimmed, []byte("[]")) {
		return -1, nil
	}
	var items []struct {
		Size int64 `json:"Size"`
	}
	if err := json.Unmarshal(trimmed, &items); err != nil {
		return -1, fmt.Errorf("parse rclone lsjson output: %w", err)
	}
	if len(items) == 0 {
		return -1, nil
	}
	return items[0].Size, nil
}

// FileSize returns the size in bytes of the remote file for h, or -1 if not found.
func (r rcloneRemote) FileSize(ctx context.Context, h hash.Hash) (int64, error) {
	out, err := r.runOutput(ctx, "lsjson", r.remotePath(h))
	if err != nil {
		if ctx.Err() != nil {
			return -1, ctx.Err()
		}
		return -1, nil // treat rclone error as "not found"
	}
	return parseLSJSONSize(out)
}
