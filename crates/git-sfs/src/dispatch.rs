//! Routes a parsed command line to core.
//!
//! This is the file phase 4 grows, one arm at a time, which is why it is not
//! part of `main.rs`: implementing a command should not touch process setup,
//! and changing process setup should not touch commands.
//!
//! Everything here is still a stub. A stub returns
//! [`Error::NotImplemented`](git_sfs_core::Error::NotImplemented) rather than
//! succeeding quietly, so the differential harness reads an unported command as
//! a failure instead of as a run that did nothing.

use git_sfs_core::{Cancel, Error, Result};

use crate::cli::{Cli, Command, SelfCommand};

/// Runs the requested command.
///
/// `_cli` carries the global flags and `_cancel` the interrupt flag; each arm
/// drops its underscore as phase 4 wires that command up.
pub fn dispatch(_cli: &Cli, command: &Command, _cancel: &Cancel) -> Result<()> {
    match command {
        Command::Help => print_help(),
        Command::Init(_) => unimplemented("init"),
        Command::Setup => unimplemented("setup"),
        Command::Add(_) => unimplemented("add"),
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

/// Writes the top-level help, as both a bare `git-sfs` and `git-sfs help` do.
pub fn print_help() -> Result<()> {
    crate::cli::print_help()
        .map_err(|err| Error::Unavailable(format!("could not write help: {err}")))
}

fn unimplemented(command: &'static str) -> Result<()> {
    Err(Error::NotImplemented { command })
}
