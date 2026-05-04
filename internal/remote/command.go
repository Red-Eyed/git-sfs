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
	"strconv"
	"strings"
	"time"

	"git-sfs/internal/errs"
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

// backendRoot returns the bare backend prefix (everything up to and including
// the first ':'), e.g. "hwr1:" from "hwr1:F:/Storage/datasets".
// Used to probe connectivity before checking a specific path.
func (r rcloneRemote) backendRoot() string {
	if i := strings.Index(r.url, ":"); i >= 0 {
		return r.url[:i+1]
	}
	return r.url
}

// checkConnectivity verifies that rclone can reach the backend at all by
// listing the backend root. A missing-path response is treated as success —
// the backend is reachable but has no files at the root yet.
func (r rcloneRemote) checkConnectivity(ctx context.Context) error {
	root := r.backendRoot()
	_, err := r.runOutput(ctx, "lsjson", root)
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	msg := strings.ToLower(err.Error())
	if strings.Contains(msg, "not found") || strings.Contains(msg, "no such file") {
		return nil
	}
	return fmt.Errorf("cannot connect to remote %s (check rclone config): %w", root, err)
}

// RequireExists verifies connectivity to the backend and then checks that the
// configured root path exists. A missing root is an error — use this before
// push/pull to prevent accidental file creation at a wrong path.
func (r rcloneRemote) RequireExists(ctx context.Context) error {
	if err := r.checkConnectivity(ctx); err != nil {
		return err
	}
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

func (r rcloneRemote) filesURL() string {
	return r.url + "/files"
}

// writeTempPathList writes one relative path per line to a temp file and returns its name.
func writeTempPathList(paths []string) (string, error) {
	f, err := os.CreateTemp("", "git-sfs-files-*.txt")
	if err != nil {
		return "", err
	}
	defer f.Close()
	for _, p := range paths {
		fmt.Fprintln(f, p)
	}
	return f.Name(), nil
}

func (r rcloneRemote) CopyToRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error {
	if len(relPaths) == 0 {
		return nil
	}
	list, err := writeTempPathList(relPaths)
	if err != nil {
		return err
	}
	defer os.Remove(list)
	return r.runCopy(ctx, "copy", "--ignore-existing", "--files-from", list, cacheFilesDir, r.filesURL())
}

func (r rcloneRemote) CopyFromRemote(ctx context.Context, cacheFilesDir string, relPaths []string) error {
	if len(relPaths) == 0 {
		return nil
	}
	list, err := writeTempPathList(relPaths)
	if err != nil {
		return err
	}
	defer os.Remove(list)
	return r.runCopy(ctx, "copy", "--ignore-existing", "--files-from", list, r.filesURL(), cacheFilesDir)
}

// runCopy runs a rclone copy command, streaming rclone's stderr to the user
// when in verbose mode (r.debug != nil) and adding --progress so transfer
// stats are visible. For non-verbose runs stderr is buffered for error messages.
func (r rcloneRemote) runCopy(ctx context.Context, args ...string) error {
	extra := []string{}
	if r.config != "" {
		extra = append(extra, "--config", r.config)
	}
	if r.debug != nil {
		extra = append(extra, "--progress")
	}
	return runCopyWithRetry(ctx, r.debug, r.retryMax, "rclone", append(extra, args...)...)
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

// runCopyWithRetry runs a streaming rclone command (no captured stdout) with
// exponential backoff. rclone's stderr is written directly to debug so the
// user sees progress; on failure the exit error is returned as-is (the user
// already saw any stderr output).
func runCopyWithRetry(ctx context.Context, debug io.Writer, retryMax int, name string, args ...string) error {
	if retryMax <= 0 {
		retryMax = 3
	}
	backoff := time.Second
	var lastErr error
	for attempt := 1; attempt <= retryMax; attempt++ {
		err := runStream(ctx, debug, name, args...)
		if err == nil {
			return nil
		}
		if ctx.Err() != nil {
			return ctx.Err()
		}
		lastErr = err
		if attempt < retryMax {
			if debug != nil {
				fmt.Fprintf(debug, "retry %d/%d after %s: %v\n", attempt, retryMax, backoff, err)
			}
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-time.After(backoff):
			}
			backoff *= 2
		}
	}
	return lastErr
}

// runStream runs a command, streaming stderr to debug (when non-nil) instead
// of buffering it. Stdout is discarded. Used for rclone copy where captured
// output is not needed but live progress output is desirable.
func runStream(ctx context.Context, debug io.Writer, name string, args ...string) error {
	if debug != nil {
		fmt.Fprintln(debug, "run:", shellQuote(append([]string{name}, args...)))
	}
	cmd := exec.CommandContext(ctx, name, args...)
	var stderr bytes.Buffer
	if debug != nil {
		cmd.Stderr = debug
	} else {
		cmd.Stderr = &stderr
	}
	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if debug == nil {
			if msg := strings.TrimSpace(stderr.String()); msg != "" {
				return fmt.Errorf("%w: %s", err, msg)
			}
		}
		return err
	}
	return nil
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
