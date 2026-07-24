//! The command-line grammar.
//!
//! This module describes the shape of `argv` and nothing else: no I/O, no
//! defaulting that depends on the filesystem, no behavior. What each command
//! then does lives in [`crate::dispatch`], so growing the tool does not touch
//! this file and changing an argument does not touch that one.
//!
//! The surface is kept compatible with v1's because `test/workflows/` drives
//! both binaries through the same invocations. Human-readable help text is not
//! frozen (contract-spec 12) and is free to improve.

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};

/// The parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "git-sfs",
    about = "store large file bytes outside Git while Git tracks symlinks",
    // `--version` is handled by hand: clap's built-in flag prints
    // "git-sfs <version>", and contract-spec 11 requires the bare tag.
    disable_version_flag = true,
    // v1 has its own `help` *subcommand* (`Command::Help` below), so clap's
    // auto-generated one would collide with it under the same name.
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Flags accepted before or after the command name.
    #[command(flatten)]
    pub global: Global,

    /// The command to run, absent when git-sfs is invoked bare.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Flags every command inherits.
///
/// `global = true` reproduces kong's inheritance, so `git-sfs push --verbose`
/// and `git-sfs --verbose push` are the same invocation. Scripts in the wild
/// use both.
#[derive(Debug, Args)]
pub struct Global {
    /// dataset config path
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        default_value = ".git-sfs/config.toml"
    )]
    pub config: Utf8PathBuf,

    /// max parallel jobs (0 = auto)
    #[arg(
        short = 'j',
        long,
        global = true,
        default_value_t = 0,
        value_name = "N"
    )]
    pub jobs: usize,

    /// verbose output
    #[arg(long, global = true)]
    pub verbose: bool,

    /// quiet output
    #[arg(long, global = true)]
    pub quiet: bool,

    /// print version
    #[arg(long, global = true)]
    pub version: bool,
}

/// The commands git-sfs accepts.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// initialize git-sfs in this repository
    Init(InitArgs),

    /// bind local cache and restore symlinks (run after clone)
    Setup(SetupArgs),

    /// cache files and replace them with symlinks
    Add(AddArgs),

    /// move a tracked file and its symlink
    Mv(MvArgs),

    /// bring files from outside the repository into git-sfs tracking
    Import(ImportArgs),

    /// check integrity and exit non-zero on failure (use in CI)
    Verify(VerifyArgs),

    /// show file sizes and cache/remote presence (always exits 0)
    Status(StatusArgs),

    /// list configured remotes
    Remotes(RemotesArgs),

    /// upload referenced cache files to the remote
    Push(PushArgs),

    /// download missing files from the remote
    Pull(PullArgs),

    /// diagnose configuration and remote problems
    Doctor(DoctorArgs),

    /// manage the git-sfs installation
    #[command(name = "self", subcommand)]
    SelfCmd(SelfCommand),

    /// print LLM-friendly reference document
    #[command(name = "llms-txt")]
    LlmsTxt,

    /// show usage
    Help,
}

/// Subcommands of `git-sfs self`.
#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    /// update git-sfs and rclone to the latest release
    Update,
}

/// Arguments to `git-sfs init`.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// overwrite an existing config
    #[arg(long)]
    pub force: bool,

    /// bind this cache directory
    #[arg(long, value_name = "PATH")]
    pub cache: Option<Utf8PathBuf>,
}

/// Arguments to `git-sfs setup`.
#[derive(Debug, Args)]
pub struct SetupArgs {
    /// bind this cache directory
    #[arg(long, value_name = "PATH")]
    pub cache: Option<Utf8PathBuf>,
}

/// Arguments to `git-sfs add`.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// files to cache
    #[arg(value_name = "PATH", required = true, num_args = 1..)]
    pub paths: Vec<Utf8PathBuf>,
}

/// Arguments to `git-sfs mv`.
#[derive(Debug, Args)]
pub struct MvArgs {
    /// source path
    #[arg(value_name = "SOURCE")]
    pub source: Utf8PathBuf,

    /// destination path
    #[arg(value_name = "DEST")]
    pub dest: Utf8PathBuf,
}

/// Arguments to `git-sfs import`.
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// delete source files after caching (default: copy, leave source intact)
    #[arg(long = "move")]
    pub move_source: bool,

    /// follow source symlinks
    #[arg(short = 'L', long)]
    pub follow_symlinks: bool,

    /// source path
    #[arg(value_name = "SOURCE")]
    pub source: Utf8PathBuf,

    /// destination path
    #[arg(value_name = "DEST")]
    pub dest: Utf8PathBuf,
}

/// Arguments to `git-sfs verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// remote name (default: "default")
    #[arg(short = 'r', long = "remote", value_name = "NAME")]
    pub remote: Option<String>,

    /// check remote files (the default)
    #[arg(long = "check-remote", overrides_with = "no_check_remote")]
    check_remote: bool,

    /// skip remote checks
    #[arg(long = "no-check-remote", overrides_with = "check_remote")]
    no_check_remote: bool,

    /// recalculate hashes for local cache and remote files
    #[arg(long)]
    pub with_integrity: bool,

    /// re-hash every file in the local cache to detect bit rot (ignores path)
    #[arg(long)]
    pub rehash: bool,

    /// limit --rehash to N randomly chosen cache files (0 = all)
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub rehash_sample: usize,

    /// path to verify
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: Utf8PathBuf,
}

impl VerifyArgs {
    /// Whether to check the remote. On unless `--no-check-remote` is given.
    ///
    /// The two flags override each other, so the last one on the command line
    /// wins and `--no-check-remote --check-remote` re-enables the check. The
    /// pair is private and reachable only through here, so no caller can read
    /// one flag and forget the other.
    #[must_use]
    pub fn check_remote(&self) -> bool {
        !self.no_check_remote
    }
}

/// Arguments to `git-sfs status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// check presence and sizes against this remote (metadata only, no download)
    ///
    /// Absent means no network call is made at all, which is what makes
    /// `status` usable offline (contract-spec 9.1).
    #[arg(long = "remote", value_name = "NAME")]
    pub remote: Option<String>,

    /// emit machine-readable JSON
    #[arg(long)]
    pub json: bool,

    /// path to inspect
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: Utf8PathBuf,
}

/// Arguments to `git-sfs remotes`.
#[derive(Debug, Args)]
pub struct RemotesArgs {
    /// emit machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments to `git-sfs push`.
#[derive(Debug, Args)]
pub struct PushArgs {
    /// remote name (default: "default")
    #[arg(short = 'r', long = "remote", value_name = "NAME")]
    pub remote: Option<String>,

    /// upload the files that are cached instead of failing on missing ones
    /// (leaves the remote incomplete)
    #[arg(long)]
    pub skip_missing: bool,

    /// path to push
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: Utf8PathBuf,
}

/// Arguments to `git-sfs pull`.
#[derive(Debug, Args)]
pub struct PullArgs {
    /// remote name (default: "default")
    #[arg(short = 'r', long = "remote", value_name = "NAME")]
    pub remote: Option<String>,

    /// path to pull
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: Utf8PathBuf,
}

/// Arguments to `git-sfs doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// remote name (default: "default")
    #[arg(short = 'r', long = "remote", value_name = "NAME")]
    pub remote: Option<String>,
}

/// Writes the top-level help to stdout.
///
/// Used for a bare `git-sfs` and for `git-sfs help`, both of which v1 answered
/// with a hand-maintained usage block. Rendering clap's own help instead means
/// the listing cannot drift from the grammar above.
pub fn print_help() -> std::io::Result<()> {
    use clap::CommandFactory;

    Cli::command().print_help()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("should parse")
    }

    #[test]
    fn grammar_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn bare_invocation_has_no_command() {
        assert!(parse(&["git-sfs"]).command.is_none());
    }

    /// kong inherits global flags, so both orderings appear in scripts and in
    /// `test/workflows/`. clap only does this with `global = true`, and
    /// dropping it would break the trailing form silently.
    #[test]
    fn global_flags_are_accepted_on_either_side_of_the_command() {
        assert!(parse(&["git-sfs", "--verbose", "push"]).global.verbose);
        assert!(parse(&["git-sfs", "push", "--verbose"]).global.verbose);
    }

    #[test]
    fn config_defaults_to_the_tracked_path() {
        assert_eq!(
            parse(&["git-sfs", "status"]).global.config,
            ".git-sfs/config.toml"
        );
    }

    #[test]
    fn path_arguments_default_to_the_current_directory() {
        let Some(Command::Status(args)) = parse(&["git-sfs", "status"]).command else {
            panic!("expected status");
        };
        assert_eq!(args.path, ".");
    }

    #[test]
    fn remote_checking_is_on_unless_switched_off() {
        fn verify(args: &[&str]) -> VerifyArgs {
            let Some(Command::Verify(args)) = parse(args).command else {
                panic!("expected verify");
            };
            args
        }

        assert!(verify(&["git-sfs", "verify"]).check_remote());
        assert!(!verify(&["git-sfs", "verify", "--no-check-remote"]).check_remote());
        assert!(verify(&["git-sfs", "verify", "--check-remote"]).check_remote());
    }

    /// `overrides_with` makes the flags last-wins rather than conflicting, so a
    /// wrapper script can append an override to a command line it did not build.
    #[test]
    fn the_last_remote_checking_flag_wins() {
        fn verify(args: &[&str]) -> VerifyArgs {
            let Some(Command::Verify(args)) = parse(args).command else {
                panic!("expected verify");
            };
            args
        }

        assert!(
            !verify(&["git-sfs", "verify", "--check-remote", "--no-check-remote"]).check_remote()
        );
        assert!(
            verify(&["git-sfs", "verify", "--no-check-remote", "--check-remote"]).check_remote()
        );
    }

    #[test]
    fn add_takes_one_or_more_paths() {
        let Some(Command::Add(args)) = parse(&["git-sfs", "add", "a.bin", "b.bin"]).command else {
            panic!("expected add");
        };
        assert_eq!(args.paths, ["a.bin", "b.bin"]);

        assert!(Cli::try_parse_from(["git-sfs", "add"]).is_err());
    }

    #[test]
    fn cache_binding_belongs_to_init_and_setup_only() {
        let Some(Command::Init(args)) = parse(&["git-sfs", "init", "--cache", "/cache"]).command
        else {
            panic!("expected init");
        };
        assert_eq!(args.cache, Some(Utf8PathBuf::from("/cache")));

        let Some(Command::Setup(args)) = parse(&["git-sfs", "setup", "--cache", "/cache"]).command
        else {
            panic!("expected setup");
        };
        assert_eq!(args.cache, Some(Utf8PathBuf::from("/cache")));

        assert!(Cli::try_parse_from(["git-sfs", "--cache", "/cache", "add", "f"]).is_err());
        assert!(Cli::try_parse_from(["git-sfs", "add", "--cache", "/cache", "f"]).is_err());
    }

    /// `move` is a Rust keyword, so the flag name has to be set by hand and a
    /// rename of the field would otherwise silently rename the flag.
    #[test]
    fn import_accepts_its_flags_under_the_v1_names() {
        let Some(Command::Import(args)) =
            parse(&["git-sfs", "import", "--move", "-L", "src", "dst"]).command
        else {
            panic!("expected import");
        };
        assert!(args.move_source);
        assert!(args.follow_symlinks);
        assert_eq!(args.source, "src");
        assert_eq!(args.dest, "dst");
    }

    #[test]
    fn self_update_is_reachable_as_two_words() {
        assert!(matches!(
            parse(&["git-sfs", "self", "update"]).command,
            Some(Command::SelfCmd(SelfCommand::Update))
        ));
    }

    #[test]
    fn llms_txt_keeps_its_hyphenated_name() {
        assert!(matches!(
            parse(&["git-sfs", "llms-txt"]).command,
            Some(Command::LlmsTxt)
        ));
    }

    /// Every command v1 exposes must still parse; a missing one would only show
    /// up as a workflow-suite failure much later.
    #[test]
    fn every_v1_command_parses() {
        let invocations: &[&[&str]] = &[
            &["git-sfs", "init"],
            &["git-sfs", "setup"],
            &["git-sfs", "add", "f"],
            &["git-sfs", "mv", "a", "b"],
            &["git-sfs", "import", "a", "b"],
            &["git-sfs", "verify"],
            &["git-sfs", "status"],
            &["git-sfs", "remotes"],
            &["git-sfs", "push"],
            &["git-sfs", "pull"],
            &["git-sfs", "doctor"],
            &["git-sfs", "self", "update"],
            &["git-sfs", "llms-txt"],
            &["git-sfs", "help"],
        ];

        for invocation in invocations {
            assert!(
                Cli::try_parse_from(*invocation).is_ok(),
                "{invocation:?} did not parse"
            );
        }
    }

    /// Paths reach `config.toml` and `status --json`, so a non-UTF-8 argument
    /// is rejected at the boundary rather than becoming a lossy string later.
    #[test]
    fn non_utf8_paths_are_rejected_at_parse_time() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![0x66, 0x80, 0x6f]);
        let args = [OsString::from("git-sfs"), OsString::from("add"), invalid];

        assert!(Cli::try_parse_from(args).is_err());
    }
}
