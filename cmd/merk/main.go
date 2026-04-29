package main

import (
	"context"
	"fmt"
	"os"

	"merk/internal/cli"
)

func main() {
	if err := cli.Run(context.Background(), os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "merk:", err)
		os.Exit(1)
	}
}
