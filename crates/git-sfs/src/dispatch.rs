//! Routes a parsed command line to core.
//!
//! Command routing stays separate from process setup: implementing a command
//! should not touch signal handling, parse-error reporting, or exit-code setup.

use camino::Utf8PathBuf;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::time::{Duration, SystemTime};

use git_sfs_core::domain::{
    Config, DEFAULT_REMOTE_NAME, RemoteConfig, check_git_sfs_version, check_rclone_version,
    compose_remote_url, tmp_dir,
};
use git_sfs_core::exec::add;
use git_sfs_core::exec::doctor::{self, DoctorReport};
use git_sfs_core::exec::import::{self, ImportOptions};
use git_sfs_core::exec::init as init_cmd;
use git_sfs_core::exec::mv;
use git_sfs_core::exec::pull;
use git_sfs_core::exec::push;
use git_sfs_core::exec::remotes;
use git_sfs_core::exec::setup as setup_cmd;
use git_sfs_core::exec::status;
use git_sfs_core::exec::verify::{self, VerifyError};
use git_sfs_core::ports::{
    FsRepo, FsStore, Lock, LockName, RcloneRemote, Remote, detect_rclone_version, discover_repo,
    purge_stale_tmp_files, resolve_cache_root,
};
use git_sfs_core::{Cancel, Error, Result};

use crate::cli::{
    AddArgs, Cli, Command, DoctorArgs, ImportArgs, InitArgs, MvArgs, PullArgs, PushArgs,
    RemotesArgs, SelfCommand, SetupArgs, StatusArgs, VerifyArgs,
};
use crate::progress::{ProgressRemote, ProgressRepo, ProgressStore, with_spinner};
use crate::reporting::{self, RenderMode};

const PULL_TMP_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Runs the requested command.
pub fn dispatch(cli: &Cli, command: &Command, cancel: &Cancel) -> Result<()> {
    match command {
        Command::Help => print_help(),
        Command::Init(args) => run_init(cli, args),
        Command::Setup(args) => run_setup(cli, args, cancel),
        Command::Add(args) => run_add(cli, args, cancel),
        Command::Mv(args) => run_mv(cli, args, cancel),
        Command::Import(args) => run_import(cli, args, cancel),
        Command::Verify(args) => run_verify(cli, args, cancel),
        Command::Status(args) => run_status(cli, args, cancel),
        Command::Remotes(args) => run_remotes(cli, args),
        Command::Push(args) => run_push(cli, args, cancel),
        Command::Pull(args) => run_pull(cli, args, cancel),
        Command::Doctor(args) => run_doctor(cli, args, cancel),
        Command::SelfCmd(SelfCommand::Update(args)) => {
            if args.pre {
                crate::self_update::run_including_prereleases(cli.global.quiet)
            } else {
                crate::self_update::run(cli.global.quiet)
            }
        }
        Command::LlmsTxt => print_llms_txt(),
    }
}

/// `git-sfs init` — create committed project metadata and bind local cache.
fn run_init(cli: &Cli, args: &InitArgs) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let outcome = with_spinner(!cli.global.quiet, "initializing git-sfs", || {
        init_cmd::init(&repo, &config_path, args.cache.as_deref(), args.force)
    })?;
    reporting::init_outcome(&outcome, RenderMode::from_quiet(cli.global.quiet));
    Ok(())
}

/// `git-sfs setup` — bind clone-local cache state.
fn run_setup(cli: &Cli, args: &SetupArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;

    let outcome = with_spinner(!cli.global.quiet, "setting up cache", || {
        setup_cmd::setup(&repo, args.cache.as_deref(), cancel)
    })?;
    reporting::setup_outcome(&outcome, RenderMode::from_quiet(cli.global.quiet));
    Ok(())
}

/// `git-sfs add <path>...` — hashes each regular file under the given paths,
/// stores it in the cache, and replaces it with a git-sfs symlink.
///
/// Requires a cache already bound via `.git-sfs/cache`; `init` and `setup`
/// are the only commands that create or change that binding.
fn run_add(cli: &Cli, args: &AddArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let cache_root = resolve_cache_root(&repo)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let store = FsStore::new(cache_root);
    let repo_port = FsRepo::new(repo.clone());
    let _lock = with_spinner(!cli.global.quiet, "waiting for add lock", || {
        Lock::acquire(&locks_dir, LockName::Add, cancel)
    })?;
    let mode = RenderMode::from_quiet(cli.global.quiet);

    match with_spinner(!cli.global.quiet, "adding files", || {
        add::add(&repo_port, &store, &repo, &args.paths, cancel)
    }) {
        Ok(outcome) => {
            reporting::add_outcome(&outcome, mode);
            Ok(())
        }
        Err(failure) => {
            reporting::add_outcome(&failure.outcome, mode);
            Err((*failure.error).into())
        }
    }
}

/// `git-sfs mv <source> <dest>` — moves a git-sfs symlink (or a directory of
/// them) and rewrites the relative targets for their new location. Never
/// touches the cache, so unlike `add` this needs no cache resolution and no
/// lock.
fn run_mv(cli: &Cli, args: &MvArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let repo_port = FsRepo::new(repo.clone());
    let mode = RenderMode::from_quiet(cli.global.quiet);

    match with_spinner(!cli.global.quiet, "moving links", || {
        mv::mv(&repo_port, &repo, &args.source, &args.dest, cancel)
    }) {
        Ok(moved) => {
            reporting::moved_links(&moved, mode);
            Ok(())
        }
        Err(failure) => {
            reporting::moved_links(&failure.moved, mode);
            Err((*failure.error).into())
        }
    }
}

/// `git-sfs import <source> <dest>` — ingests an external file or directory
/// into the cache and creates git-sfs symlinks at `dest`. Requires an
/// already-bound cache and takes the `import` lock, like `add` and unlike
/// `mv`.
fn run_import(cli: &Cli, args: &ImportArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let cache_root = resolve_cache_root(&repo)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let store = FsStore::new(cache_root);
    let _lock = with_spinner(!cli.global.quiet, "waiting for import lock", || {
        Lock::acquire(&locks_dir, LockName::Import, cancel)
    })?;
    let mode = RenderMode::from_quiet(cli.global.quiet);

    let options = ImportOptions {
        move_source: args.move_source,
        follow_symlinks: args.follow_symlinks,
    };
    match with_spinner(!cli.global.quiet, "importing files", || {
        import::import(
            &store,
            &repo,
            &cwd,
            &args.source,
            &args.dest,
            options,
            cancel,
        )
    }) {
        Ok(outcome) => {
            reporting::import_outcome(&outcome, mode);
            Ok(())
        }
        Err(failure) => {
            reporting::import_outcome(&failure.outcome, mode);
            Err((*failure.error).into())
        }
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
    let cache_root = resolve_cache_root(&repo)?;

    let store = ProgressStore::new(FsStore::new(cache_root.clone()), !cli.global.quiet);
    let repo_port = ProgressRepo::new(FsRepo::new(repo), !cli.global.quiet);
    let remote = match args.remote.as_deref() {
        Some(name) => Some(build_progress_remote(
            cli,
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
        reporting::remotes_json(&entries)
    } else {
        reporting::remotes_text(&entries);
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
    let cache_root = resolve_cache_root(&repo)?;

    let remote_name = args.remote.as_deref().unwrap_or(DEFAULT_REMOTE_NAME);
    let remote =
        build_progress_remote(cli, &config, remote_name, config_path.parent(), &cache_root)?;
    remote.require_exists(cancel)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let _lock = with_spinner(!cli.global.quiet, "waiting for push lock", || {
        Lock::acquire(&locks_dir, LockName::Push, cancel)
    })?;
    let store = ProgressStore::new(FsStore::new(cache_root.clone()), !cli.global.quiet);
    let repo_port = ProgressRepo::new(FsRepo::new(repo), !cli.global.quiet);
    let cache_files_dir = cache_root.join("files");
    let mode = RenderMode::from_quiet(cli.global.quiet);

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
            reporting::push_outcome(&outcome, mode);
            Ok(())
        }
        Err(failure) => {
            reporting::push_outcome(&failure.outcome, mode);
            Err((*failure.error).into())
        }
    }
}

/// `git-sfs pull` — download referenced remote objects missing from the cache.
fn run_pull(cli: &Cli, args: &PullArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;
    let cache_root = resolve_cache_root(&repo)?;
    purge_pull_tmp(&cache_root)?;

    let remote_name = args.remote.as_deref().unwrap_or(DEFAULT_REMOTE_NAME);
    let remote =
        build_progress_remote(cli, &config, remote_name, config_path.parent(), &cache_root)?;
    remote.require_exists(cancel)?;

    let locks_dir = git_sfs_core::domain::locks_dir(&cache_root);
    let _lock = with_spinner(!cli.global.quiet, "waiting for pull lock", || {
        Lock::acquire(&locks_dir, LockName::Pull, cancel)
    })?;
    let store = ProgressStore::new(FsStore::new(cache_root.clone()), !cli.global.quiet);
    let repo_port = ProgressRepo::new(FsRepo::new(repo), !cli.global.quiet);
    let cache_files_dir = cache_root.join("files");

    let outcome = pull::pull(
        &repo_port,
        &store,
        &remote,
        &cache_files_dir,
        &args.path,
        cancel,
    )?;
    reporting::pull_outcome(&outcome, RenderMode::from_quiet(cli.global.quiet));
    Ok(())
}

fn purge_pull_tmp(cache_root: &camino::Utf8Path) -> Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(PULL_TMP_STALE_AFTER)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    purge_stale_tmp_files(cache_root, cutoff)?;
    Ok(())
}

/// `git-sfs doctor` — diagnose repository, cache, rclone, and remote setup.
fn run_doctor(cli: &Cli, args: &DoctorArgs, cancel: &Cancel) -> Result<()> {
    let mut report = DoctorReport::new();
    println!();

    let repo = match check_value(&mut report, "git repository", || {
        let cwd = current_dir_utf8().map_err(|err| err.to_string())?;
        let repo = discover_repo(&cwd).map_err(|err| err.to_string())?;
        Ok((repo.to_string(), repo))
    }) {
        Some(repo) => repo,
        None => {
            report.skip_all(&[
                "git-sfs config",
                "git-sfs version",
                "git core.symlinks",
                "cache config",
                "cache directory",
                "cache permissions",
                "rclone binary",
                "rclone version",
            ]);
            return finish_doctor(report);
        }
    };

    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = match check_value(&mut report, "git-sfs config", || {
        let config = load_config(&config_path).map_err(|err| err.to_string())?;
        Ok((config_path.to_string(), config))
    }) {
        Some(config) => config,
        None => {
            report.skip_all(&[
                "git-sfs version",
                "git core.symlinks",
                "cache config",
                "cache directory",
                "cache permissions",
                "rclone binary",
                "rclone version",
            ]);
            return finish_doctor(report);
        }
    };

    check_git_sfs_version_for_doctor(&mut report, &config);
    check_core_symlinks(&mut report, &repo);

    let cache_root = match check_value(&mut report, "cache config", || {
        let cache_root = resolve_cache_root(&repo).map_err(|err| err.to_string())?;
        Ok((cache_root.to_string(), cache_root))
    }) {
        Some(cache_root) => cache_root,
        None => {
            report.skip_all(&[
                "cache directory",
                "cache permissions",
                "rclone binary",
                "rclone version",
            ]);
            return finish_doctor(report);
        }
    };

    check_cache_directory(&mut report, &cache_root);
    check_cache_permissions(&mut report, &cache_root);

    match find_rclone_binary() {
        Ok(path) => report.pass("rclone binary", path.to_string()),
        Err(err) => {
            report.fail("rclone binary", err);
            report.skip("rclone version");
            return finish_doctor(report);
        }
    }
    check_rclone_version_for_doctor(&mut report, &config, cancel);
    check_doctor_remotes(
        &mut report,
        &config,
        args.remote.as_deref(),
        &config_path,
        &cache_root,
        cancel,
    );
    finish_doctor(report)
}

fn check_value<T>(
    report: &mut DoctorReport,
    label: &str,
    check: impl FnOnce() -> std::result::Result<(String, T), String>,
) -> Option<T> {
    match check() {
        Ok((detail, value)) => {
            report.pass(label, detail);
            Some(value)
        }
        Err(err) => {
            report.fail(label, err);
            None
        }
    }
}

fn check_git_sfs_version_for_doctor(report: &mut DoctorReport, config: &Config) {
    let version = crate::version::VERSION;
    match &config.settings.min_git_sfs_version {
        Some(minimum) => match check_git_sfs_version(version, minimum) {
            Ok(()) => report.pass("git-sfs version", format!("{version} (min: {minimum})")),
            Err(err) => report.fail("git-sfs version", err.to_string()),
        },
        None => report.pass("git-sfs version", version),
    }
}

fn check_core_symlinks(report: &mut DoctorReport, repo: &camino::Utf8Path) {
    match git_core_symlinks(repo) {
        Ok(CoreSymlinks::True { explicit }) => {
            let detail = if explicit { "true" } else { "true (default)" };
            report.pass("git core.symlinks", detail);
        }
        Ok(CoreSymlinks::False) => {
            report.fail("git core.symlinks", "core.symlinks is false");
        }
        Err(err) => report.fail("git core.symlinks", err),
    }
}

fn check_cache_directory(report: &mut DoctorReport, cache_root: &camino::Utf8Path) {
    let result = (|| {
        let metadata = std::fs::metadata(cache_root)
            .map_err(|err| format!("does not exist or is unreadable: {cache_root}: {err}"))?;
        if !metadata.is_dir() {
            return Err(format!("not a directory: {cache_root}"));
        }
        let cache_tmp = tmp_dir(cache_root);
        std::fs::create_dir_all(&cache_tmp)
            .map_err(|err| format!("create cache tmp {cache_tmp}: {err}"))?;
        let (_file, path) = create_probe_file(&cache_tmp, ".git-sfs-doctor")
            .map_err(|err| format!("not writable: {err}"))?;
        cleanup_probe_file(&path);
        Ok(format!("{cache_root} (tmp writable)"))
    })();
    match result {
        Ok(detail) => report.pass("cache directory", detail),
        Err(err) => report.fail("cache directory", err),
    }
}

fn check_cache_permissions(report: &mut DoctorReport, cache_root: &camino::Utf8Path) {
    let cache_tmp = tmp_dir(cache_root);
    let result = (|| {
        std::fs::create_dir_all(&cache_tmp)
            .map_err(|err| format!("create cache tmp {cache_tmp}: {err}"))?;
        let (mut file, path) = create_probe_file(&cache_tmp, ".git-sfs-doctor-mode")?;
        file.write_all(b"git-sfs doctor\n")
            .map_err(|err| format!("write mode probe {path}: {err}"))?;
        let mut permissions = file
            .metadata()
            .map_err(|err| format!("stat mode probe {path}: {err}"))?
            .permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&path, permissions)
            .map_err(|err| format!("chmod mode probe {path}: {err}"))?;
        let mode = std::fs::metadata(&path)
            .map_err(|err| format!("restat mode probe {path}: {err}"))?
            .permissions()
            .mode();
        cleanup_probe_file(&path);
        if mode & 0o222 == 0 {
            Ok("read-only mode preserved".to_owned())
        } else {
            Err("filesystem does not preserve read-only mode bits".to_owned())
        }
    })();
    match result {
        Ok(detail) => report.pass("cache permissions", detail),
        Err(err) => report.fail("cache permissions", err),
    }
}

fn check_rclone_version_for_doctor(report: &mut DoctorReport, config: &Config, cancel: &Cancel) {
    match detect_rclone_version(cancel) {
        Ok(version) => match &config.settings.min_rclone_version {
            Some(minimum) => match check_rclone_version(&version, minimum) {
                Ok(()) => report.pass("rclone version", format!("v{version} (min: {minimum})")),
                Err(err) => report.fail("rclone version", err.to_string()),
            },
            None => report.pass("rclone version", format!("v{version}")),
        },
        Err(err) => report.fail("rclone version", err.to_string()),
    }
}

fn check_doctor_remotes(
    report: &mut DoctorReport,
    config: &Config,
    remote_filter: Option<&str>,
    config_path: &camino::Utf8Path,
    cache_root: &camino::Utf8Path,
    cancel: &Cancel,
) {
    let config_dir = config_path.parent();
    for name in doctor::remote_names(config, remote_filter) {
        report.section(format!("remote: {name}"));
        let Some(remote_config) = config.remotes.get(name.as_str()) else {
            report.fail("config", format!("remote {name:?} is not configured"));
            continue;
        };
        if !check_rclone_config_file(report, remote_config, config_dir) {
            report.skip_all(&["remote backend", "remote path"]);
            continue;
        }
        let remote = match build_rclone_remote(config, &name, config_dir, cache_root) {
            Ok(remote) => remote,
            Err(err) => {
                report.fail("remote backend", err.to_string());
                report.skip("remote path");
                continue;
            }
        };
        if let Err(err) = remote.check_backend(cancel) {
            report.fail("remote backend", err.to_string());
            report.skip("remote path");
            continue;
        }
        report.pass("remote backend", format!("{}:", remote_config.backend));
        match remote.check_path(cancel) {
            Ok(()) => report.pass("remote path", remote_url_for_doctor(remote_config)),
            Err(err) => report.fail("remote path", err.to_string()),
        }
    }
}

fn check_rclone_config_file(
    report: &mut DoctorReport,
    remote_config: &RemoteConfig,
    config_dir: Option<&camino::Utf8Path>,
) -> bool {
    let Some(config_path) = &remote_config.rclone_config_path else {
        report.pass(
            "rclone config file",
            "using rclone default (~/.config/rclone/rclone.conf)",
        );
        return true;
    };
    let resolved = resolve_rclone_config_path(config_dir, config_path);
    if resolved.is_file() {
        report.pass("rclone config file", resolved.to_string());
        true
    } else {
        report.fail("rclone config file", format!("file not found: {resolved}"));
        false
    }
}

fn remote_url_for_doctor(remote_config: &RemoteConfig) -> String {
    compose_remote_url(
        &remote_config.backend,
        remote_config.path.as_deref().unwrap_or(""),
    )
}

fn finish_doctor(report: DoctorReport) -> Result<()> {
    reporting::doctor_report(&report);
    if report.has_failures() {
        return Err(Error::Unavailable(format!(
            "doctor: {} check(s) failed",
            report.failed()
        )));
    }
    Ok(())
}

enum CoreSymlinks {
    True { explicit: bool },
    False,
}

fn git_core_symlinks(repo: &camino::Utf8Path) -> std::result::Result<CoreSymlinks, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["config", "--get", "--bool", "core.symlinks"])
        .output()
        .map_err(|err| format!("run git config: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        return match stdout.as_str() {
            "true" => Ok(CoreSymlinks::True { explicit: true }),
            "false" => Ok(CoreSymlinks::False),
            other => Err(format!("unexpected core.symlinks value: {other:?}")),
        };
    }
    if output.status.code() == Some(1) && stdout.is_empty() {
        return Ok(CoreSymlinks::True { explicit: false });
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(format!("git config failed: {stderr}"))
}

fn find_rclone_binary() -> std::result::Result<Utf8PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or_else(|| "PATH is not set".to_owned())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("rclone");
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Utf8PathBuf::from_path_buf(candidate)
                .map_err(|path| format!("rclone path is not valid UTF-8: {}", path.display()));
        }
    }
    Err("rclone not found on PATH: install from https://rclone.org/downloads/".to_owned())
}

fn create_probe_file(
    dir: &camino::Utf8Path,
    prefix: &str,
) -> std::result::Result<(std::fs::File, Utf8PathBuf), String> {
    let pid = std::process::id();
    for attempt in 0..100u32 {
        let path = dir.join(format!("{prefix}-{pid}-{attempt}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("{path}: {err}")),
        }
    }
    Err(format!("could not create a unique probe file in {dir}"))
}

fn cleanup_probe_file(path: &camino::Utf8Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

/// `git-sfs verify` — strict local and optional remote integrity checks.
fn run_verify(cli: &Cli, args: &VerifyArgs, cancel: &Cancel) -> Result<()> {
    let cwd = current_dir_utf8()?;
    let repo = discover_repo(&cwd)?;
    let config_path = resolved_config_path(&repo, &cli.global.config);
    let config = load_config(&config_path)?;
    check_git_sfs_floor(&config)?;
    let cache_root = resolve_cache_root(&repo)?;

    let store = ProgressStore::new(FsStore::new(cache_root.clone()), !cli.global.quiet);
    let repo_port = ProgressRepo::new(FsRepo::new(repo), !cli.global.quiet);
    let remote = if args.check_remote() {
        let remote_name = args.remote.as_deref().unwrap_or(DEFAULT_REMOTE_NAME);
        let remote =
            build_progress_remote(cli, &config, remote_name, config_path.parent(), &cache_root)?;
        remote.require_exists(cancel)?;
        Some(remote)
    } else {
        None
    };

    match verify::verify(
        &repo_port,
        &store,
        remote.as_ref().map(|remote| remote as &dyn Remote),
        &tmp_dir(&cache_root),
        &args.path,
        args.with_integrity || args.rehash || args.rehash_sample > 0,
        cancel,
    ) {
        Ok(report) => {
            reporting::verify_success(&report, RenderMode::from_quiet(cli.global.quiet));
            Ok(())
        }
        Err(error @ VerifyError::Failed { .. }) => {
            if let Some(report) = error.report() {
                reporting::verify_report(report);
            }
            Err(error.into())
        }
        Err(error) => Err(error.into()),
    }
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

fn build_progress_remote(
    cli: &Cli,
    config: &Config,
    name: &str,
    config_dir: Option<&camino::Utf8Path>,
    cache_root: &camino::Utf8Path,
) -> Result<ProgressRemote<RcloneRemote>> {
    let remote = build_rclone_remote(config, name, config_dir, cache_root)?
        .with_transfer_progress(!cli.global.quiet);
    Ok(ProgressRemote::new(remote, !cli.global.quiet))
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

const LLMS_TXT: &str = include_str!("../../../llms.txt");

fn print_llms_txt() -> Result<()> {
    std::io::stdout()
        .write_all(llms_txt().as_bytes())
        .map_err(|err| Error::Unavailable(format!("could not write llms.txt: {err}")))
}

fn llms_txt() -> &'static str {
    LLMS_TXT
}

#[cfg(test)]
mod tests {
    use git_sfs_core::exec::doctor::DoctorStatus;

    use super::*;

    fn check<'a>(report: &'a DoctorReport, label: &str) -> &'a DoctorStatus {
        report
            .sections()
            .iter()
            .flat_map(|section| section.checks())
            .find(|check| check.label() == label)
            .unwrap_or_else(|| panic!("missing doctor check: {label}"))
            .status()
    }

    fn git(repo: &camino::Utf8Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("git failed to start: {err}"));
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn embedded_llms_txt_is_the_generated_git_sfs_reference() {
        let text = llms_txt();

        assert!(text.starts_with("# git-sfs\n"));
        assert!(text.contains("git-sfs llms-txt"));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn doctor_checks_environmental_assumptions_where_they_are_chosen() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(repo_dir.path().to_owned()).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "core.symlinks", "false"]);

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = Utf8PathBuf::from_path_buf(cache_dir.path().to_owned()).unwrap();
        let mut report = DoctorReport::new();

        check_core_symlinks(&mut report, &repo);
        check_cache_directory(&mut report, &cache);
        check_cache_permissions(&mut report, &cache);

        assert!(matches!(
            check(&report, "git core.symlinks"),
            DoctorStatus::Fail { detail } if detail == "core.symlinks is false"
        ));
        assert!(matches!(
            check(&report, "cache directory"),
            DoctorStatus::Pass { detail } if detail.contains("tmp writable")
        ));
        assert!(matches!(
            check(&report, "cache permissions"),
            DoctorStatus::Pass { detail } if detail == "read-only mode preserved"
        ));
        assert!(
            cache.join("tmp").is_dir(),
            "doctor should choose and prepare the cache tmp directory it probes"
        );
        assert_eq!(
            std::fs::read_dir(cache.join("tmp")).unwrap().count(),
            0,
            "doctor probe files should be cleaned up"
        );
    }
}
