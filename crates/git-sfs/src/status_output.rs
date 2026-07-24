//! Terminal and JSON rendering for `git-sfs status`.
//!
//! The report itself belongs to core; the shape users see on stdout belongs
//! here in the binary crate.

use git_sfs_core::exec::status::{self, RemoteState, StatusFile, StatusReport};
use git_sfs_core::{Error, Result};
use serde::Serialize;

/// Prints a human-readable status report.
pub fn print_text(report: &StatusReport, verbose: bool) {
    println!("tracked symlinks: {}", report.tracked);
    println!("unique files: {}", report.unique_files);
    println!("cached locally: {}", report.cached);
    println!("missing locally: {}", report.missing_local);
    println!("total size: {}", humanize_bytes(report.total_size));
    if report.remote_checked {
        println!("on remote: {}", report.on_remote.unwrap_or(0));
        println!("unpushed: {}", report.unpushed.unwrap_or(0));
        println!("remote unknown: {}", report.remote_unknown.unwrap_or(0));
    }
    if report.files.is_empty() {
        return;
    }
    println!("details:");
    for file in &report.files {
        println!(
            "{}",
            format_status_line(file, report.remote_checked, verbose)
        );
    }
}

fn format_status_line(file: &StatusFile, remote_checked: bool, verbose: bool) -> String {
    let size = if file.size == status::SIZE_UNKNOWN {
        "-".to_owned()
    } else {
        humanize_bytes(file.size as u64)
    };
    let mut line = format!(
        "{}: {} local={}",
        file.path,
        size,
        if file.cached { "cached" } else { "missing" }
    );
    if remote_checked {
        line.push_str(" remote=");
        line.push_str(&remote_word(file.remote.as_ref()));
    }
    if verbose {
        line.push(' ');
        line.push_str(&file.hash.to_string());
    }
    line
}

fn remote_word(remote: Option<&RemoteState>) -> String {
    match remote {
        Some(RemoteState::Present) => "present".to_owned(),
        Some(RemoteState::Absent) => "missing".to_owned(),
        Some(RemoteState::Unknown { cause }) => format!("unknown ({cause})"),
        None => "unchecked".to_owned(),
    }
}

/// Prints a JSON status report.
pub fn print_json(report: &StatusReport) -> Result<()> {
    let payload = StatusJson {
        tracked: report.tracked,
        unique_files: report.unique_files,
        cached: report.cached,
        missing_local: report.missing_local,
        total_size: report.total_size,
        remote_checked: report.remote_checked,
        on_remote: report.on_remote,
        unpushed: report.unpushed,
        remote_unknown: report.remote_unknown,
        files: report
            .files
            .iter()
            .map(|file| StatusFileJson {
                path: file.path.to_string(),
                hash: file.hash.to_string(),
                size: file.size,
                cached: file.cached,
                remote: file.remote.as_ref().map(remote_json),
            })
            .collect(),
    };
    serde_json::to_writer_pretty(std::io::stdout(), &payload)
        .map_err(|err| Error::Unavailable(format!("could not write status JSON: {err}")))?;
    println!();
    Ok(())
}

#[derive(Serialize)]
struct StatusJson<'a> {
    tracked: usize,
    unique_files: usize,
    cached: usize,
    missing_local: usize,
    total_size: u64,
    remote_checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_remote: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unpushed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_unknown: Option<usize>,
    files: Vec<StatusFileJson<'a>>,
}

#[derive(Serialize)]
struct StatusFileJson<'a> {
    path: String,
    hash: String,
    size: i64,
    cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<RemoteStateJson<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RemoteStateJson<'a> {
    Present,
    Absent,
    Unknown { cause: &'a str },
}

fn remote_json(remote: &RemoteState) -> RemoteStateJson<'_> {
    match remote {
        RemoteState::Present => RemoteStateJson::Present,
        RemoteState::Absent => RemoteStateJson::Absent,
        RemoteState::Unknown { cause } => RemoteStateJson::Unknown { cause },
    }
}

fn humanize_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n as f64;
    let mut unit = UNITS[0];
    for candidate in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = candidate;
    }
    if unit == "B" {
        format!("{n} B")
    } else {
        format!("{value:.1} {unit}")
    }
}
