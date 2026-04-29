package cli

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/vadstup/merk/internal/core"
)

type options struct {
	cache   string
	config  string
	verbose bool
	quiet   bool
}

func Run(ctx context.Context, args []string) error {
	return run(ctx, args, os.Stdout, os.Stderr)
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	fs := flag.NewFlagSet("merk", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var opts options
	fs.StringVar(&opts.cache, "cache", "", "cache directory")
	fs.StringVar(&opts.config, "config", "dataset.yaml", "dataset config path")
	fs.BoolVar(&opts.verbose, "verbose", false, "verbose output")
	fs.BoolVar(&opts.quiet, "quiet", false, "quiet output")
	if err := fs.Parse(args); err != nil {
		return err
	}
	rest := fs.Args()
	if len(rest) == 0 {
		usage(stdout)
		return nil
	}

	app := core.App{
		Stdout:     stdout,
		Stderr:     stderr,
		CacheFlag:  opts.cache,
		ConfigPath: opts.config,
		Quiet:      opts.quiet,
		Verbose:    opts.verbose,
	}

	cmd, cmdArgs := rest[0], rest[1:]
	switch cmd {
	case "init":
		return app.Init(ctx, has(cmdArgs, "--force"))
	case "setup":
		return app.Setup(ctx)
	case "add":
		if len(cmdArgs) == 0 {
			return fmt.Errorf("add requires at least one path")
		}
		return app.Add(ctx, cmdArgs)
	case "status":
		return app.Status(ctx)
	case "verify":
		return app.Verify(ctx)
	case "push":
		remote := ""
		if len(cmdArgs) > 0 {
			remote = cmdArgs[0]
		}
		return app.Push(ctx, remote)
	case "pull":
		path := "."
		if len(cmdArgs) > 0 {
			path = cmdArgs[0]
		}
		return app.Pull(ctx, path)
	case "materialize":
		path := "."
		if len(cmdArgs) > 0 {
			path = cmdArgs[0]
		}
		return app.Materialize(ctx, path)
	case "dematerialize":
		path := "."
		if len(cmdArgs) > 0 {
			path = cmdArgs[0]
		}
		return app.Dematerialize(ctx, path)
	case "gc":
		gfs := flag.NewFlagSet("gc", flag.ContinueOnError)
		gfs.SetOutput(stderr)
		var dryRun, worktreeOnly, objects bool
		gfs.BoolVar(&dryRun, "dry-run", false, "show what would be deleted")
		gfs.BoolVar(&worktreeOnly, "worktree-only", false, "remove unused worktree links")
		gfs.BoolVar(&objects, "objects", false, "remove unreferenced cache objects")
		if err := gfs.Parse(cmdArgs); err != nil {
			return err
		}
		return app.GC(ctx, core.GCOptions{DryRun: dryRun, WorktreeOnly: worktreeOnly, Objects: objects})
	case "help", "-h", "--help":
		usage(stdout)
		return nil
	default:
		return fmt.Errorf("unknown command %q", cmd)
	}
}

func has(args []string, want string) bool {
	for _, arg := range args {
		if arg == want {
			return true
		}
	}
	return false
}

func usage(w io.Writer) {
	fmt.Fprintln(w, "usage: merk [--cache path] [--config path] <command> [args]")
	fmt.Fprintln(w, "commands: init setup add status verify push pull materialize dematerialize gc")
}
