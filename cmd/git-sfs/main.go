package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"git-sfs/internal/cli"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := cli.Run(ctx, os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "git-sfs:", err)
		os.Exit(1)
	}
}
