package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"git-sfs/internal/cli"
	"git-sfs/internal/errs"
)

// exitCode maps an error to a stable exit code:
//
//	0    success
//	1    config or usage error
//	2    I/O or remote error (default for unknown errors)
//	3    integrity failure (corrupt or permission-wrong data)
//	130  canceled by the user (SIGINT/SIGTERM)
func exitCode(err error) int {
	if err == nil {
		return 0
	}
	switch {
	// A user interrupt takes precedence: Ctrl-C should always read as canceled,
	// not as whatever partial error the aborted operation happened to return.
	case errors.Is(err, context.Canceled):
		return 130
	case errors.Is(err, errs.ErrInvalidConfig),
		errors.Is(err, errs.ErrMissingCacheConfig):
		return 1
	case errors.Is(err, errs.ErrCorruptCachedFile),
		errors.Is(err, errs.ErrCorruptRemoteFile),
		errors.Is(err, errs.ErrWrongCachePermissions),
		errors.Is(err, errs.ErrInvalidSymlink):
		return 3
	default:
		return 2
	}
}

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := cli.Run(ctx, os.Args[1:]); err != nil {
		if errors.Is(err, context.Canceled) {
			fmt.Fprintln(os.Stderr, "git-sfs: canceled")
			os.Exit(130)
		}
		fmt.Fprintln(os.Stderr, "git-sfs:", err)
		os.Exit(exitCode(err))
	}
}
