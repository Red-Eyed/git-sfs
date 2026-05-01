package cli

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"

	"git-sfs/internal/core"
	"git-sfs/internal/version"
)

type options struct {
	cache   string
	config  string
	verbose bool
	quiet   bool
	version bool
}

func Run(ctx context.Context, args []string) error {
	return run(ctx, args, os.Stdout, os.Stderr)
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	fs := flag.NewFlagSet("git-sfs", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var opts options
	fs.StringVar(&opts.cache, "cache", "", "cache directory")
	fs.StringVar(&opts.config, "config", ".git-sfs/config.toml", "dataset config path")
	fs.BoolVar(&opts.verbose, "verbose", false, "verbose output")
	fs.BoolVar(&opts.quiet, "quiet", false, "quiet output")
	fs.BoolVar(&opts.version, "version", false, "print version")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if opts.version {
		fmt.Fprintln(stdout, version.Version)
		return nil
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
	if opts.verbose {
		fmt.Fprintf(stderr, "debug: command=%s args=%q\n", cmd, cmdArgs)
	}
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
	case "import":
		ifs := flag.NewFlagSet("import", flag.ContinueOnError)
		ifs.SetOutput(stderr)
		var followSymlinks bool
		ifs.BoolVar(&followSymlinks, "L", false, "follow source symlinks")
		if err := ifs.Parse(cmdArgs); err != nil {
			return err
		}
		cmdArgs = ifs.Args()
		if len(cmdArgs) != 2 {
			return fmt.Errorf("%s requires source and destination", cmd)
		}
		return app.ImportWithOptions(ctx, cmdArgs[0], cmdArgs[1], core.ImportOptions{FollowSymlinks: followSymlinks})
	case "verify":
		vfs := flag.NewFlagSet("verify", flag.ContinueOnError)
		vfs.SetOutput(stderr)
		var remote bool
		var withIntegrity bool
		vfs.BoolVar(&remote, "remote", true, "check remote files")
		vfs.BoolVar(&withIntegrity, "with-integrity", false, "recalculate hashes for local cache and remote files")
		if err := vfs.Parse(cmdArgs); err != nil {
			return err
		}
		path := "."
		if len(vfs.Args()) > 0 {
			path = vfs.Args()[0]
		}
		return app.Verify(ctx, remote, withIntegrity, path)
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
	fmt.Fprintln(w, "usage: git-sfs [--cache path] [--config path] <command> [args]")
	fmt.Fprintln(w, "commands:")
	fmt.Fprintln(w, "  init")
	fmt.Fprintln(w, "  setup")
	fmt.Fprintln(w, "  add")
	fmt.Fprintln(w, "  import")
	fmt.Fprintln(w, "  verify")
	fmt.Fprintln(w, "  push")
	fmt.Fprintln(w, "  pull")
	fmt.Fprintln(w, "  help")
}
