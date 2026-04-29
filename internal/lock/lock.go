package lock

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type Lock struct {
	path string
}

func Acquire(ctx context.Context, dir, name string) (*Lock, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, name+".lock")
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
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("waiting for lock %s: %w", path, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}

func (l *Lock) Release() error {
	if l == nil {
		return nil
	}
	return os.RemoveAll(l.path)
}
