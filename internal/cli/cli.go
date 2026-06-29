package cli

import (
	"context"
	"fmt"
	"io"
	"os"

	"strings"

	"github.com/alecthomas/kong"

	"git-sfs/internal/core"
	"git-sfs/internal/version"
)

// grammar is the typed command-line surface. kong maps argv onto these fields,
// validating types and positional arity, and exposes per-command --help. Global
// flags live at the top level and are inherited by every subcommand, so they may
// appear before or after the command name (e.g. "git-sfs push --verbose").
type grammar struct {
	Cache   string `help:"cache directory" placeholder:"PATH"`
	Config  string `help:"dataset config path" default:".git-sfs/config.toml" placeholder:"PATH"`
	Jobs    int    `short:"j" help:"max parallel jobs (0 = auto)"`
	Verbose bool   `help:"verbose output"`
	Quiet   bool   `help:"quiet output"`

	Init    initCmd    `cmd:"" help:"initialize git-sfs in this repository"`
	Setup   setupCmd   `cmd:"" help:"bind local cache and restore symlinks (run after clone)"`
	Add     addCmd     `cmd:"" help:"cache files and replace them with symlinks"`
	Mv      mvCmd      `cmd:"" help:"move a tracked file and its symlink"`
	Import  importCmd  `cmd:"" help:"bring files from outside the repository into git-sfs tracking"`
	Verify  verifyCmd  `cmd:"" help:"check integrity and exit non-zero on failure (use in CI)"`
	Status  statusCmd  `cmd:"" help:"show file sizes and cache/remote presence (always exits 0)"`
	Remotes remotesCmd `cmd:"" help:"list configured remotes"`
	Push    pushCmd    `cmd:"" help:"upload referenced cache files to the remote"`
	Pull    pullCmd    `cmd:"" help:"download missing files from the remote"`
	Doctor  doctorCmd  `cmd:"" help:"diagnose configuration and remote problems"`
	Self    selfCmd    `cmd:"" help:"manage the git-sfs installation"`
	LLMsTxt llmsTxtCmd `cmd:"" name:"llms-txt" help:"print LLM-friendly reference document"`
	Help    helpCmd    `cmd:"" help:"show usage"`
}

type selfCmd struct {
	Update selfUpdateCmd `cmd:"" help:"update git-sfs and rclone to the latest release"`
}

type selfUpdateCmd struct{}

type initCmd struct {
	Force bool `help:"overwrite an existing config"`
}

type setupCmd struct{}

type addCmd struct {
	// Optional at the kong level so the empty case yields our own clear error
	// rather than kong's generic "expected <path>" message.
	Paths []string `arg:"" optional:"" name:"path" help:"files to cache"`
}

type mvCmd struct {
	Source string `arg:"" help:"source path"`
	Dest   string `arg:"" help:"destination path"`
}

type importCmd struct {
	Move           bool   `help:"delete source files after caching (default: copy, leave source intact)"`
	FollowSymlinks bool   `short:"L" name:"follow-symlinks" help:"follow source symlinks"`
	Source         string `arg:"" help:"source path"`
	Dest           string `arg:"" help:"destination path"`
}

type verifyCmd struct {
	RemoteName    string `short:"r" name:"remote" help:"remote name (default: \"default\")"`
	CheckRemote   bool   `name:"check-remote" negatable:"" default:"true" help:"check remote files"`
	WithIntegrity bool   `name:"with-integrity" help:"recalculate hashes for local cache and remote files"`
	Rehash        bool   `name:"rehash" help:"re-hash every file in the local cache to detect bit rot (ignores path arg)"`
	RehashSample  int    `name:"rehash-sample" help:"limit --rehash to N randomly chosen cache files (0 = all)" placeholder:"N"`
	Path          string `arg:"" optional:"" default:"." help:"path to verify"`
}

type statusCmd struct {
	Remote string `name:"remote" help:"check presence and sizes against this remote (metadata only, no download)" placeholder:"NAME"`
	JSON   bool   `name:"json" help:"emit machine-readable JSON"`
	Path   string `arg:"" optional:"" default:"." help:"path to inspect"`
}

type remotesCmd struct {
	JSON bool `name:"json" help:"emit machine-readable JSON"`
}

type pushCmd struct {
	RemoteName string `short:"r" name:"remote" help:"remote name (default: \"default\")"`
}

type pullCmd struct {
	RemoteName string `short:"r" name:"remote" help:"remote name (default: \"default\")"`
	Path       string `arg:"" optional:"" default:"." help:"path to pull"`
}

type doctorCmd struct {
	RemoteName string `short:"r" name:"remote" help:"remote name (default: \"default\")"`
}

type llmsTxtCmd struct{}

type helpCmd struct{}

func Run(ctx context.Context, args []string) error {
	return run(ctx, args, os.Stdout, os.Stderr)
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	// Command-less invocations are handled before kong so they need no
	// subcommand: --version prints the version, and a bare call prints usage.
	if has(args, "--version") {
		fmt.Fprintln(stdout, version.Version)
		return nil
	}
	if len(args) == 0 {
		usage(stdout)
		return nil
	}

	var g grammar
	parser, err := kong.New(&g,
		kong.Name("git-sfs"),
		kong.Description("store large file bytes outside Git while Git tracks symlinks"),
		kong.Writers(stdout, stderr),
		// Never terminate the process; errors propagate to main, which owns the
		// exit-code mapping. kong still prints --help output during Parse.
		kong.Exit(func(int) {}),
	)
	if err != nil {
		return err
	}
	kctx, parseErr := parser.Parse(args)
	// kong prints help during Parse when --help/-h is present; stop here so a
	// command is not also executed.
	if has(args, "--help") || has(args, "-h") {
		return nil
	}
	if parseErr != nil {
		return parseErr
	}

	cmd := commandPath(kctx.Command())
	if g.Verbose {
		fmt.Fprintf(stderr, "debug: command=%s\n", cmd)
	}

	app := core.App{
		Stdout:     stdout,
		Stderr:     stderr,
		CacheFlag:  g.Cache,
		ConfigPath: g.Config,
		Jobs:       g.Jobs,
		Quiet:      g.Quiet,
		Verbose:    g.Verbose,
	}
	return dispatch(ctx, app, g, cmd, stdout)
}

func dispatch(ctx context.Context, app core.App, g grammar, cmd string, stdout io.Writer) error {
	switch cmd {
	case "init":
		return app.Init(ctx, g.Init.Force)
	case "setup":
		return app.Setup(ctx)
	case "add":
		if len(g.Add.Paths) == 0 {
			return fmt.Errorf("add requires at least one path")
		}
		return app.Add(ctx, g.Add.Paths)
	case "mv":
		return app.Mv(g.Mv.Source, g.Mv.Dest)
	case "import":
		opts := core.ImportOptions{FollowSymlinks: g.Import.FollowSymlinks, Move: g.Import.Move}
		return app.ImportWithOptions(ctx, g.Import.Source, g.Import.Dest, opts)
	case "verify":
		if g.Verify.Rehash {
			return app.RehashCache(ctx, g.Verify.RehashSample)
		}
		return app.Verify(ctx, g.Verify.RemoteName, g.Verify.CheckRemote, g.Verify.WithIntegrity, g.Verify.Path)
	case "status":
		return app.Status(ctx, g.Status.Remote, g.Status.JSON, g.Status.Path)
	case "remotes":
		return app.Remotes(g.Remotes.JSON)
	case "push":
		return app.Push(ctx, g.Push.RemoteName)
	case "pull":
		return app.Pull(ctx, g.Pull.RemoteName, g.Pull.Path)
	case "doctor":
		return app.Doctor(ctx, g.Doctor.RemoteName)
	case "self update":
		return selfUpdate(ctx, stdout, app.Stderr, app.Quiet)
	case "llms-txt":
		return printLLMsTxt(stdout)
	case "help":
		usage(stdout)
		return nil
	default:
		return fmt.Errorf("unknown command %q", cmd)
	}
}

// commandPath strips positional-arg placeholders from kctx.Command() output
// (e.g. "add <path>" → "add", "self update" → "self update").
func commandPath(full string) string {
	var parts []string
	for _, p := range strings.Fields(full) {
		if !strings.HasPrefix(p, "<") {
			parts = append(parts, p)
		}
	}
	return strings.Join(parts, " ")
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
	fmt.Fprintln(w, "usage: git-sfs [global flags] <command> [args]")
	fmt.Fprintln(w, "")
	fmt.Fprintln(w, "global flags:")
	fmt.Fprintln(w, "  --cache PATH    cache directory")
	fmt.Fprintln(w, "  --config PATH   dataset config path (default .git-sfs/config.toml)")
	fmt.Fprintln(w, "  -j, --jobs N    max parallel jobs (0 = auto)")
	fmt.Fprintln(w, "  --verbose       verbose output")
	fmt.Fprintln(w, "  --quiet         quiet output")
	fmt.Fprintln(w, "  --version       print version")
	fmt.Fprintln(w, "")
	fmt.Fprintln(w, "commands:")
	fmt.Fprintln(w, "  init    [--force]")
	fmt.Fprintln(w, "  setup")
	fmt.Fprintln(w, "  add     <path>...")
	fmt.Fprintln(w, "  mv      <src> <dst>")
	fmt.Fprintln(w, "  import  [--move] [-L] <src> <dst>")
	fmt.Fprintln(w, "  verify  [-r NAME] [--no-check-remote] [--with-integrity] [path]")
	fmt.Fprintln(w, "  status  [-r NAME] [--json] [path]")
	fmt.Fprintln(w, "  remotes [--json]")
	fmt.Fprintln(w, "  push    [-r NAME]")
	fmt.Fprintln(w, "  pull    [-r NAME] [path]")
	fmt.Fprintln(w, "  doctor       [-r NAME]")
	fmt.Fprintln(w, "  self update")
	fmt.Fprintln(w, "  llms-txt")
	fmt.Fprintln(w, "  help")
	fmt.Fprintln(w, "")
	fmt.Fprintln(w, "run 'git-sfs <command> --help' for command-specific flags")
}
