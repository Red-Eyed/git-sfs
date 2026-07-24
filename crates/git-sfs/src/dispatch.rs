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
use git_sfs_core::domain::{
    Config, DEFAULT_REMOTE_NAME, RemoteConfig, check_git_sfs_version, compose_remote_url, tmp_dir,
};
use git_sfs_core::exec::add::{self, AddOutcome};
use git_sfs_core::exec::import::{self, ImportOptions, ImportOutcome};
use git_sfs_core::exec::mv::{self, MovedLink};
use git_sfs_core::exec::pull::{self, PullOutcome};
use git_sfs_core::exec::push::{self, PushOutcome};
use git_sfs_core::exec::remotes::{self, RemoteEntry};
use git_sfs_core::exec::status;
use git_sfs_core::ports::{
    FsRepo, FsStore, Lock, LockName, RcloneRemote, Remote, discover_repo, resolve_cache_root,
};
use git_sfs_core::{Cancel, Error, Result};
use serde::Serialize;

use crate::cli::{
    AddArgs, Cli, Command, ImportArgs, MvArgs, PullArgs, PushArgs, RemotesArgs, SelfCommand,
    StatusArgs,
};

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
        Command::Mv(args) => run_mv(args, cancel),
        Command::Import(args) => run_import(cli, args, cancel),
        Command::Verify(_) => unimplemented("verify"),
        Command::Status(args) => run_status(cli, args, cancel),
        Command::Remotes(args) => run_remotes(cli, args),
        Command::Push(args) => run_push(cli, args, cancel),
        Command::Pull(args) => run_pull(cli, args, cancel),
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
    let cwd = current_dir_utf8()?;
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

/// `git-sfs mv <source> <dest>` — moves a git-sfs symlink (or a directory of
/// them) and rewrites the relative targets for their new location. Never
/// touches the cache, so unlike `add` this needs no cache resolution and no
/// lock (contract-spec §3.3; v1's `mv.go` takes no lock either).
fn run_mv(args: &MvArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let repo_port = FsRepo::new(repo.clone());

    match mv::mv(&repo_port, &repo, &args.source, &args.dest, cancel) {
        Ok(moved) => {
            print_moved(&moved);
            Ok(())
        }
        Err(failure) => {
            print_moved(&failure.moved);
            Err((*failure.error).into())
        }
    }
}

/// Prints each relocated link, matching v1's per-link `moved <old> -> <new>`
/// line (`mv.go:61,114`), though the exact wording is unfrozen (contract-spec
/// 12).
fn print_moved(moved: &[MovedLink]) {
    for link in moved {
        println!("moved {} -> {}", link.old_path, link.new_path);
    }
}

/// `git-sfs import <source> <dest>` — ingests an external file or directory
/// into the cache and creates git-sfs symlinks at `dest`. Requires an
/// already-bound cache and takes the `import` lock, like `add` and unlike
/// `mv` (contract-spec §3.3 exempts `mv` alone from touching the cache).
fn run_import(cli: &Cli, args: &ImportArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let cache_root = resolve_cache_root(
        &repo,
        cli.global.cache.as_deref(),
        std::env::var("GIT_SFS_CACHE").ok().as_deref(),
    )?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let store = FsStore::new(cache_root);
    let _lock = Lock::acquire(&locks_dir, LockName::Import, cancel)?;

    let options = ImportOptions {
        move_source: args.move_source,
        follow_symlinks: args.follow_symlinks,
    };
    match import::import(
        &store,
        &repo,
        &cwd,
        &args.source,
        &args.dest,
        options,
        cancel,
    ) {
        Ok(outcome) => {
            print_import_outcome(&outcome);
            Ok(())
        }
        Err(failure) => {
            print_import_outcome(&failure.outcome);
            Err((*failure.error).into())
        }
    }
}

/// Prints each imported file and any skipped-for-encoding candidates,
/// matching v1's per-file `imported <src> -> <dst> -> <hash>` line
/// (`import.go:101`), though the exact wording is unfrozen (contract-spec
/// 12).
fn print_import_outcome(outcome: &ImportOutcome) {
    for file in &outcome.imported {
        println!("imported {} -> {} -> {}", file.src, file.dst, file.hash);
    }
    for description in &outcome.unrepresentable {
        eprintln!("git-sfs: warning: skipped {description} (not a valid UTF-8 path)");
    }
}

/// `git-sfs status` — inspect tracked symlinks and cache/remote metadata
/// without moving bytes.
fn run_status(cli: &Cli, args: &StatusArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;
    let cache_root = resolve_cache_root(
        &repo,
        cli.global.cache.as_deref(),
        std::env::var("GIT_SFS_CACHE").ok().as_deref(),
    )?;

    let store = FsStore::new(cache_root.clone());
    let repo_port = FsRepo::new(repo);
    let remote = match args.remote.as_deref() {
        Some(name) => Some(build_rclone_remote(
            &config,
            name,
            config_path.parent(),
            &cache_root,
        )?),
        None => None,
    };
    let report = status::status(
        &repo_port,
        &store,
        remote.as_ref().map(|r| r as &dyn Remote),
        &args.path,
        cancel,
    )?;

    if args.json {
        crate::status_output::print_json(&report)
    } else {
        crate::status_output::print_text(&report, cli.global.verbose);
        Ok(())
    }
}

/// `git-sfs remotes` — list configured remotes from the committed
/// `.git-sfs/config.toml` only. This never contacts rclone; `doctor` owns
/// connectivity checks.
fn run_remotes(cli: &Cli, args: &RemotesArgs) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let entries = remotes::remotes(&config_path)?;

    if args.json {
        print_remotes_json(&entries)
    } else {
        print_remotes_text(&entries);
        Ok(())
    }
}

/// `git-sfs push` — upload referenced cache objects to the configured remote.
fn run_push(cli: &Cli, args: &PushArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;
    let cache_root = resolve_cache_root(
        &repo,
        cli.global.cache.as_deref(),
        std::env::var("GIT_SFS_CACHE").ok().as_deref(),
    )?;

    let remote_name = args.remote.as_deref().unwrap_or(DEFAULT_REMOTE_NAME);
    let remote = build_rclone_remote(&config, remote_name, config_path.parent(), &cache_root)?;
    remote.require_exists(cancel)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let _lock = Lock::acquire(&locks_dir, LockName::Push, cancel)?;
    let store = FsStore::new(cache_root.clone());
    let repo_port = FsRepo::new(repo);
    let cache_files_dir = cache_root.join("files");

    match push::push(
        &repo_port,
        &store,
        &remote,
        &cache_files_dir,
        &args.path,
        args.skip_missing,
        cancel,
    ) {
        Ok(outcome) => {
            print_push_outcome(&outcome, cli.global.quiet);
            Ok(())
        }
        Err(failure) => {
            print_push_outcome(&failure.outcome, cli.global.quiet);
            Err((*failure.error).into())
        }
    }
}

fn print_push_outcome(outcome: &PushOutcome, quiet: bool) {
    print_skipped_push_objects(outcome);
    if !quiet && !outcome.uploaded.is_empty() {
        println!(
            "push: uploaded {} file(s) to remote",
            outcome.uploaded.len()
        );
    }
}

fn print_skipped_push_objects(outcome: &PushOutcome) {
    if outcome.skipped.is_empty() {
        return;
    }

    const MAX_WARNINGS: usize = 10;
    let mut skipped_paths = outcome
        .skipped
        .iter()
        .flat_map(|object| {
            object
                .paths
                .iter()
                .map(move |path| (path.to_owned(), object.hash))
        })
        .collect::<Vec<_>>();
    skipped_paths.sort_by(|left, right| left.0.cmp(&right.0));

    eprintln!(
        "git-sfs: warning: push skipped {} missing cached object(s)",
        outcome.skipped.len()
    );
    for (path, hash) in skipped_paths.iter().take(MAX_WARNINGS) {
        eprintln!("  {path} ({})", hash.short());
    }
    if skipped_paths.len() > MAX_WARNINGS {
        eprintln!(
            "  ... {} more path(s) skipped",
            skipped_paths.len() - MAX_WARNINGS
        );
    }
    eprintln!("  run: git-sfs pull <path> to restore them");
}

/// `git-sfs pull` — download referenced remote objects missing from the cache.
fn run_pull(cli: &Cli, args: &PullArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;
    let cache_root = resolve_cache_root(
        &repo,
        cli.global.cache.as_deref(),
        std::env::var("GIT_SFS_CACHE").ok().as_deref(),
    )?;

    let remote_name = args.remote.as_deref().unwrap_or(DEFAULT_REMOTE_NAME);
    let remote = build_rclone_remote(&config, remote_name, config_path.parent(), &cache_root)?;
    remote.require_exists(cancel)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let _lock = Lock::acquire(&locks_dir, LockName::Pull, cancel)?;
    let store = FsStore::new(cache_root.clone());
    let repo_port = FsRepo::new(repo);
    let cache_files_dir = cache_root.join("files");

    let outcome = pull::pull(
        &repo_port,
        &store,
        &remote,
        &cache_files_dir,
        &args.path,
        cancel,
    )?;
    print_pull_outcome(&outcome, cli.global.quiet);
    Ok(())
}

fn print_pull_outcome(outcome: &PullOutcome, quiet: bool) {
    if !quiet && !outcome.downloaded.is_empty() {
        println!(
            "pull: downloaded {} file(s) from remote",
            outcome.downloaded.len()
        );
    }
}

fn print_remotes_text(entries: &[RemoteEntry]) {
    println!("remotes: {}", entries.len());
    for entry in entries {
        println!("{}", format_remote_line(entry));
    }
}

fn format_remote_line(entry: &RemoteEntry) -> String {
    let mut line = format!("{}: backend={}", entry.name, entry.backend);
    if let Some(path) = &entry.path {
        line.push_str(" path=");
        line.push_str(path);
    }
    if let Some(config) = &entry.config {
        line.push_str(" config=");
        line.push_str(config);
    }
    if entry.default {
        line.push_str(" (default)");
    }
    line
}

#[derive(Serialize)]
struct RemotesJson<'a> {
    remotes: &'a [RemoteEntry],
}

fn print_remotes_json(entries: &[RemoteEntry]) -> Result<()> {
    let payload = RemotesJson { remotes: entries };
    serde_json::to_writer_pretty(std::io::stdout(), &payload)
        .map_err(|err| Error::Unavailable(format!("could not write remotes JSON: {err}")))?;
    println!();
    Ok(())
}

fn load_config(config_path: &camino::Utf8Path) -> Result<Config> {
    let text = std::fs::read_to_string(config_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::Config(format!(
                "config file not found: {config_path} (run git-sfs init)"
            ))
        } else {
            Error::Unavailable(format!("open config {config_path}: {err}"))
        }
    })?;
    git_sfs_core::domain::config::parse_and_validate(&text)
        .map_err(|err| Error::Config(err.to_string()))
}

fn check_git_sfs_floor(config: &Config) -> Result<()> {
    let Some(minimum) = &config.settings.min_git_sfs_version else {
        return Ok(());
    };
    check_git_sfs_version(crate::version::VERSION, minimum)
        .map_err(|err| Error::Config(format!("git-sfs version check failed: {err}")))
}

fn build_rclone_remote(
    config: &Config,
    name: &str,
    config_dir: Option<&camino::Utf8Path>,
    cache_root: &camino::Utf8Path,
) -> Result<RcloneRemote> {
    let remote_config = config
        .remotes
        .get(name)
        .ok_or_else(|| Error::Config(format!("remote {name:?} is not configured")))?;
    let mut remote = rclone_remote_from_config(remote_config, config_dir, cache_root);
    if let Some(retry_max) = config.settings.retry_max
        && let Ok(retry_max) = u32::try_from(retry_max)
        && retry_max > 0
    {
        remote = remote.with_retry_max(retry_max);
    }
    Ok(remote)
}

fn rclone_remote_from_config(
    remote_config: &RemoteConfig,
    config_dir: Option<&camino::Utf8Path>,
    cache_root: &camino::Utf8Path,
) -> RcloneRemote {
    let url = compose_remote_url(
        &remote_config.backend,
        remote_config.path.as_deref().unwrap_or(""),
    );
    let mut remote = RcloneRemote::new(url, tmp_dir(cache_root));
    if let Some(config_path) = &remote_config.rclone_config_path {
        remote = remote.with_config(resolve_rclone_config_path(config_dir, config_path));
    }
    remote
}

fn resolve_rclone_config_path(
    config_dir: Option<&camino::Utf8Path>,
    config_path: &camino::Utf8Path,
) -> Utf8PathBuf {
    if config_path.is_absolute() {
        config_path.to_owned()
    } else {
        config_dir
            .unwrap_or_else(|| camino::Utf8Path::new("."))
            .join(config_path)
    }
}

/// The current directory as a validated UTF-8 path — every command that
/// resolves a relative argument needs it, either against the repo root
/// (`add`, `mv`) or, for `import`'s external `src`, against the directory
/// itself.
fn current_dir_utf8() -> Result<Utf8PathBuf> {
    let cwd = std::env::current_dir().map_err(|err| {
        Error::Unavailable(format!("could not determine the current directory: {err}"))
    })?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|_| Error::Unavailable("current directory is not valid UTF-8".to_owned()))
}

fn resolved_config_path(repo: &camino::Utf8Path, config: &camino::Utf8Path) -> Utf8PathBuf {
    if config.is_absolute() {
        config.to_owned()
    } else {
        repo.join(config)
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
