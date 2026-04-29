package main

import (
	"context"
	"fmt"
	"os"

	"git-sfs/internal/cli"
)

func main() {
	if err := cli.Run(context.Background(), os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "git-sfs:", err)
		os.Exit(1)
	}
}
