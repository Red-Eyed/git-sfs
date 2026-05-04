package lock

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"time"
)

type Lock struct {
	path string
}

// Acquire waits until the named lock is available. If the lock is held by
// another process, it prints a one-time notice to w (if non-nil) after a
// short grace period so the user knows the process is not hung.
func Acquire(ctx context.Context, dir, name string) (*Lock, error) {
	return AcquireWithWriter(ctx, dir, name, os.Stderr)
}

func AcquireWithWriter(ctx context.Context, dir, name string, w io.Writer) (*Lock, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, name+".lock")
	notified := false
	for {
		err := os.Mkdir(path, 0o755)
		if err == nil {
			meta := []byte(fmt.Sprintf("pid: %d\n", os.Getpid()))
			_ = os.WriteFile(filepath.Join(path, "owner"), meta, 0o644)
			return &Lock{path: path}, nil
		}
		if !os.IsExist(err) {
			return nil, err
		}
		if !notified && w != nil {
			owner := lockOwner(path)
			if owner != "" {
				fmt.Fprintf(w, "waiting for lock %s (held by %s)...\n", name, owner)
			} else {
				fmt.Fprintf(w, "waiting for lock %s...\n", name)
			}
			notified = true
		}
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("waiting for lock %s: %w", path, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}

// lockOwner reads the pid from the lock's owner file, returning empty string if unavailable.
func lockOwner(lockPath string) string {
	data, err := os.ReadFile(filepath.Join(lockPath, "owner"))
	if err != nil {
		return ""
	}
	return string(data[:len(data)-1]) // strip trailing newline
}

func (l *Lock) Release() error {
	if l == nil {
		return nil
	}
	return os.RemoveAll(l.path)
}
