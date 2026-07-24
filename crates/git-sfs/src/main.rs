//! git-sfs: store large file bytes outside Git while Git tracks symlinks.
//!
//! This crate is the imperative shell. It owns argv, the terminal, signals, and
//! the exit code; `git-sfs-core` owns everything else and can do none of those
//! things (rust-rewrite-plan 3).

#![warn(missing_docs)]

mod cli;
mod dispatch;
mod exit;
mod reporting;
mod status_output;
mod version;

use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;
use git_sfs_core::{Cancel, Error, Result};

use crate::cli::Cli;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_failure(&err),
    };

    let cancel = Cancel::new();
    if let Err(err) = watch_for_interrupts(&cancel) {
        return report(&err);
    }

    match outcome(&cli, &cancel) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => report(&err),
    }
}

/// The result of the run, with cancellation taking precedence.
///
/// contract-spec 9 requires Ctrl-C to read as canceled rather than as whatever
/// partial error the aborted operation produced on its way out. Applying that
/// here means it holds for every command including ones not yet written, rather
/// than depending on each of them returning the right error while unwinding.
fn outcome(cli: &Cli, cancel: &Cancel) -> Result<()> {
    let result = run(cli, cancel);
    if cancel.is_canceled() {
        return Err(Error::Canceled);
    }
    result
}

fn run(cli: &Cli, cancel: &Cancel) -> Result<()> {
    if cli.global.version {
        println!("{}", version::VERSION);
        return Ok(());
    }

    match &cli.command {
        Some(command) => dispatch::dispatch(cli, command, cancel),
        None => dispatch::print_help(),
    }
}

/// Installs the SIGINT and SIGTERM handler.
///
/// The handler only sets a flag. Work polls it and unwinds normally, so
/// temp-file cleanup and lock release happen on the ordinary path rather than
/// in a signal context.
fn watch_for_interrupts(cancel: &Cancel) -> Result<()> {
    let flag = cancel.clone();
    ctrlc::set_handler(move || flag.cancel())
        .map_err(|err| Error::Unavailable(format!("could not handle interrupts: {err}")))
}

/// contract-spec 9 freezes the `git-sfs: ` prefix and the stream. The wording
/// after the prefix is free.
fn report(error: &Error) -> ExitCode {
    eprintln!("git-sfs: {error}");
    ExitCode::from(exit::code_for(error))
}

/// Reports a clap parse failure, or the help and version output clap models as
/// one.
fn report_parse_failure(error: &clap::Error) -> ExitCode {
    // clap writes help and version to stdout and real errors to stderr, so the
    // stream is already correct here. A write that fails leaves nothing to say
    // and nowhere to say it, so it becomes the exit code instead.
    if error.print().is_err() {
        return ExitCode::from(exit::UNAVAILABLE);
    }

    match error.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => ExitCode::SUCCESS,
        _ => ExitCode::from(exit::USAGE),
    }
}
