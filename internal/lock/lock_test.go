package lock

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

func TestAcquireRelease(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "locks")
	l, err := Acquire(context.Background(), dir, "cache")
	if err != nil {
		t.Fatal(err)
	}
	if err := l.Release(); err != nil {
		t.Fatal(err)
	}
	if err := l.Release(); err != nil {
		t.Fatal(err)
	}
}

func TestAcquireWaitsForExistingLock(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "locks")
	l, err := Acquire(context.Background(), dir, "cache")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Millisecond)
	defer cancel()
	if _, err := Acquire(ctx, dir, "cache"); err == nil {
		t.Fatal("expected timeout waiting for lock")
	}
	if err := l.Release(); err != nil {
		t.Fatal(err)
	}
}
