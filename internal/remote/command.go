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
	progress io.Writer
	retryMax int
}

func NewRclone(url string) Remote {
	return NewRcloneWithOptions(url, Options{})
}

func NewRcloneWithOptions(url string, opts Options) Remote {
	return rcloneRemote{url: strings.TrimRight(url, "/"), config: opts.RcloneConfig, debug: opts.Debug, progress: opts.Progress, retryMax: opts.RetryMax}
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
		return rcloneRemote{url: remote + ":" + path, config: opts.RcloneConfig, debug: opts.Debug, progress: opts.Progress, retryMax: opts.RetryMax}
	}
	return rcloneRemote{url: remote + ":" + strings.TrimLeft(path, "/"), config: opts.RcloneConfig, debug: opts.Debug, progress: opts.Progress, retryMax: opts.RetryMax}
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

// validateConfig checks that the rclone config file exists before any rclone
// call is made. A wrong or missing path produces a clear error instead of
// letting the OS errno surface deep inside a copy command.
func (r rcloneRemote) validateConfig() error {
	if r.config == "" {
		return nil
	}
	if _, err := os.Stat(r.config); err != nil {
		return fmt.Errorf("rclone config file not found: %s", r.config)
	}
	return nil
}

// isRemotePathNotFound reports whether a rclone error indicates that a remote
// path simply does not exist (backend is reachable, directory is absent).
// It deliberately excludes errors that mention "config" or "section" — those
// are rclone configuration errors, not missing-path signals.
func isRemotePathNotFound(msg string) bool {
	if strings.Contains(msg, "config") || strings.Contains(msg, "section") {
		return false
	}
	// "no such file or directory" is an OS errno; it appears when rclone reads
	// a local path or a local backend — treat it as path-not-found only when
	// there is no config-related context in the message.
	return strings.Contains(msg, "directory not found") ||
		strings.Contains(msg, "object not found") ||
		strings.Contains(msg, "no such file or directory") ||
		strings.Contains(msg, "path not found")
}

// CheckBackend verifies that rclone can reach the backend at all by listing
// directories at the backend root. A missing-path response is treated as
// success — the backend is reachable but has no directories at the root yet.
func (r rcloneRemote) CheckBackend(ctx context.Context) error {
	root := r.backendRoot()
	_, err := r.runOutput(ctx, "lsd", root)
	if err == nil {
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	msg := strings.ToLower(err.Error())
	if isRemotePathNotFound(msg) {
		return nil
	}
	return fmt.Errorf("cannot connect to remote %s (check rclone config): %w", root, err)
}

// CheckPath verifies that the configured remote root path exists.
// Call after CheckBackend to distinguish connectivity from missing-path errors.
func (r rcloneRemote) CheckPath(ctx context.Context) error {
	out, err := r.runOutput(ctx, "lsjson", r.url)
	if err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		msg := strings.ToLower(err.Error())
		if isRemotePathNotFound(msg) {
			return fmt.Errorf("remote path does not exist: %s (create it before pushing)", r.url)
		}
		return fmt.Errorf("remote unreachable (%s): %w", r.url, err)
	}
	_ = out
	return nil
}

// RequireExists verifies connectivity to the backend and then checks that the
// configured root path exists. A missing root is an error — use this before
// push/pull to prevent accidental file creation at a wrong path.
func (r rcloneRemote) RequireExists(ctx context.Context) error {
	if err := r.validateConfig(); err != nil {
		return err
	}
	if err := r.CheckBackend(ctx); err != nil {
		return err
	}
	return r.CheckPath(ctx)
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
	// --size-only: skip files whose byte count already matches. This catches
	// partial uploads (wrong size → re-upload) without relying on modtime,
	// which many SFTP servers do not support. --ignore-existing would silently
	// skip corrupt partial uploads left by an interrupted transfer.
	return r.runCopy(ctx, "copy", "--size-only", "--files-from", list, cacheFilesDir, r.filesURL())
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

// runCopy runs a rclone copy command, streaming rclone's stderr (including its
// own --progress bar) to the user whenever output is not silenced, so transfer
// stats are visible during push and pull. --progress is added when the progress
// writer is set (i.e. not --quiet). For silenced runs stderr is buffered for
// error messages.
func (r rcloneRemote) runCopy(ctx context.Context, args ...string) error {
	extra := []string{}
	if r.config != "" {
		extra = append(extra, "--config", r.config)
	}
	if r.progress != nil {
		extra = append(extra, "--progress")
	}
	return runCopyWithRetry(ctx, r.streamTarget(), r.debug, r.retryMax, "rclone", append(extra, args...)...)
}

// streamTarget is where rclone's stderr (and its --progress bar) is sent during
// a copy. Verbose debug output takes precedence; otherwise the progress writer
// (set when not --quiet) receives it. A nil result means stderr is buffered and
// surfaced only on failure.
func (r rcloneRemote) streamTarget() io.Writer {
	if r.debug != nil {
		return r.debug
	}
	return r.progress
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
// exponential backoff. rclone's stderr is written directly to streamTo so the
// user sees progress; logTo (verbose only) receives our own command and retry
// logging. On failure the exit error is returned as-is (the user already saw
// any stderr output).
func runCopyWithRetry(ctx context.Context, streamTo, logTo io.Writer, retryMax int, name string, args ...string) error {
	if retryMax <= 0 {
		retryMax = 3
	}
	backoff := time.Second
	var lastErr error
	for attempt := 1; attempt <= retryMax; attempt++ {
		err := runStream(ctx, streamTo, logTo, name, args...)
		if err == nil {
			return nil
		}
		if ctx.Err() != nil {
			return ctx.Err()
		}
		lastErr = err
		if attempt < retryMax {
			if logTo != nil {
				fmt.Fprintf(logTo, "retry %d/%d after %s: %v\n", attempt, retryMax, backoff, err)
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

// runStream runs a command, streaming its stderr to streamTo (when non-nil)
// instead of buffering it. Stdout is discarded. logTo (when non-nil) receives
// our own "run:" command echo. Used for rclone copy where captured output is
// not needed but live progress output is desirable.
func runStream(ctx context.Context, streamTo, logTo io.Writer, name string, args ...string) error {
	if logTo != nil {
		fmt.Fprintln(logTo, "run:", shellQuote(append([]string{name}, args...)))
	}
	cmd := exec.CommandContext(ctx, name, args...)
	var stderr bytes.Buffer
	if streamTo != nil {
		// Pass the underlying *os.File when available so the child process
		// inherits the real file descriptor. rclone detects a TTY via the fd
		// and renders the --progress bar; an io.Writer pipe suppresses it.
		if f, ok := streamTo.(*os.File); ok {
			cmd.Stderr = f
		} else {
			cmd.Stderr = streamTo
		}
	} else {
		cmd.Stderr = &stderr
	}
	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if streamTo == nil {
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
