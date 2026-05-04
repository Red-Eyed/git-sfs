package main

import (
	"errors"
	"fmt"
	"testing"

	"git-sfs/internal/errs"
)

func TestExitCode(t *testing.T) {
	cases := []struct {
		err  error
		want int
	}{
		{nil, 0},
		{errs.ErrInvalidConfig, 1},
		{errs.ErrMissingCacheConfig, 1},
		{fmt.Errorf("wrapped: %w", errs.ErrInvalidConfig), 1},
		{errors.Join(errs.ErrInvalidConfig, fmt.Errorf("detail")), 1},
		{errs.ErrCorruptCachedFile, 3},
		{errs.ErrCorruptRemoteFile, 3},
		{errs.ErrWrongCachePermissions, 3},
		{errs.ErrInvalidSymlink, 3},
		{fmt.Errorf("wrapped: %w", errs.ErrCorruptCachedFile), 3},
		{errs.ErrMissingCachedFile, 2},
		{errs.ErrMissingRemoteFile, 2},
		{fmt.Errorf("some I/O error"), 2},
	}
	for _, tc := range cases {
		got := exitCode(tc.err)
		if got != tc.want {
			t.Errorf("exitCode(%v) = %d, want %d", tc.err, got, tc.want)
		}
	}
}
