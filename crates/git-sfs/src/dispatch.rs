//! Routes a parsed command line to core.
//!
//! This is the file phase 4 grows, one arm at a time, which is why it is not
//! part of `main.rs`: implementing a command should not touch process setup,
//! and changing process setup should not touch commands.
//!
//! Every unported command is still a stub. A stub returns
//! [`Error::NotImplemented`](git_sfs_core::Error::NotImplemented) rather than
//! succeeding quietly, so the differential harness reads an unported command as
//! a failure instead of as a run that did nothing.

use camino::Utf8PathBuf;
use git_sfs_core::exec::add::{self, AddOutcome};
use git_sfs_core::ports::{FsRepo, FsStore, Lock, LockName, discover_repo, resolve_cache_root};
use git_sfs_core::{Cancel, Error, Result};

use crate::cli::{AddArgs, Cli, Command, SelfCommand};

/// Runs the requested command.
///
/// `_cli` carries the global flags and `_cancel` the interrupt flag; each arm
/// drops its underscore as phase 4 wires that command up.
pub fn dispatch(cli: &Cli, command: &Command, cancel: &Cancel) -> Result<()> {
    match command {
        Command::Help => print_help(),
        Command::Init(_) => unimplemented("init"),
        Command::Setup => unimplemented("setup"),
        Command::Add(args) => run_add(cli, args, cancel),
        Command::Mv(_) => unimplemented("mv"),
        Command::Import(_) => unimplemented("import"),
        Command::Verify(_) => unimplemented("verify"),
        Command::Status(_) => unimplemented("status"),
        Command::Remotes(_) => unimplemented("remotes"),
        Command::Push(_) => unimplemented("push"),
        Command::Pull(_) => unimplemented("pull"),
        Command::Doctor(_) => unimplemented("doctor"),
        Command::SelfCmd(SelfCommand::Update) => unimplemented("self update"),
        Command::LlmsTxt => unimplemented("llms-txt"),
    }
}

/// `git-sfs add <path>...` — hashes each regular file under the given paths,
/// stores it in the cache, and replaces it with a git-sfs symlink.
///
/// Requires a cache already bound via `.git-sfs/cache` (or `--cache`/
/// `GIT_SFS_CACHE`) — this command does not create or bind one itself; see
/// `ports::local_state`'s module doc for why that stays a separate, still
/// open question.
fn run_add(cli: &Cli, args: &AddArgs, cancel: &Cancel) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|err| {
        Error::Unavailable(format!("could not determine the current directory: {err}"))
    })?;
    let cwd = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|_| Error::Unavailable("current directory is not valid UTF-8".to_owned()))?;

    let repo = discover_repo(&cwd)?;
    let cache_root = resolve_cache_root(
        &repo,
        cli.global.cache.as_deref(),
        std::env::var("GIT_SFS_CACHE").ok().as_deref(),
    )?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let store = FsStore::new(cache_root);
    let repo_port = FsRepo::new(repo.clone());
    let _lock = Lock::acquire(&locks_dir, LockName::Add, cancel)?;

    match add::add(&repo_port, &store, &repo, &args.paths, cancel) {
        Ok(outcome) => {
            print_add_outcome(&outcome);
            Ok(())
        }
        Err(failure) => {
            print_add_outcome(&failure.outcome);
            Err((*failure.error).into())
        }
    }
}

/// Prints each converted file and any skipped-for-encoding candidates, in
/// that order — matching v1's per-file `added <path> -> <hash>` line
/// (`add.go:94`), though the exact wording is unfrozen (contract-spec 12).
fn print_add_outcome(outcome: &AddOutcome) {
    for file in &outcome.added {
        println!("added {} -> {}", file.path, file.hash);
    }
    for description in &outcome.unrepresentable {
        eprintln!("git-sfs: warning: skipped {description} (not a valid UTF-8 path)");
    }
}

/// Writes the top-level help, as both a bare `git-sfs` and `git-sfs help` do.
pub fn print_help() -> Result<()> {
    crate::cli::print_help()
        .map_err(|err| Error::Unavailable(format!("could not write help: {err}")))
}

fn unimplemented(command: &'static str) -> Result<()> {
    Err(Error::NotImplemented { command })
}
