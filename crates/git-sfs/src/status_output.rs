//! Terminal and JSON rendering for `git-sfs status`.
//!
//! The report itself belongs to core; the shape users see on stdout belongs
//! here in the binary crate.

use git_sfs_core::exec::status::{
    self, RemoteAbsenceReason, RemoteState, StatusFile, StatusReport,
};
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
        Some(RemoteState::Absent { .. }) => "missing".to_owned(),
        Some(RemoteState::Unknown { cause }) => format!("unknown ({cause})"),
        None => "unchecked".to_owned(),
    }
}

/// Prints a JSON status report.
pub fn print_json(report: &StatusReport) -> Result<()> {
    let payload = status_json(report);
    serde_json::to_writer_pretty(std::io::stdout(), &payload)
        .map_err(|err| Error::Unavailable(format!("could not write status JSON: {err}")))?;
    println!();
    Ok(())
}

fn status_json(report: &StatusReport) -> StatusJson<'_> {
    StatusJson {
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
    }
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
    Absent { reason: RemoteAbsenceReasonJson },
    Unknown { cause: &'a str },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RemoteAbsenceReasonJson {
    NotListed,
}

fn remote_json(remote: &RemoteState) -> RemoteStateJson<'_> {
    match remote {
        RemoteState::Present => RemoteStateJson::Present,
        RemoteState::Absent { reason } => RemoteStateJson::Absent {
            reason: remote_absence_reason_json(*reason),
        },
        RemoteState::Unknown { cause } => RemoteStateJson::Unknown { cause },
    }
}

fn remote_absence_reason_json(reason: RemoteAbsenceReason) -> RemoteAbsenceReasonJson {
    match reason {
        RemoteAbsenceReason::NotListed => RemoteAbsenceReasonJson::NotListed,
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

#[cfg(test)]
mod tests {
    use git_sfs_core::domain::Sha256;
    use serde_json::json;

    use super::*;

    fn hash() -> Sha256 {
        Sha256::parse("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .expect("valid hash")
    }

    #[test]
    fn status_json_carries_all_three_remote_states() {
        let hash = hash();
        let report = StatusReport {
            tracked: 3,
            unique_files: 3,
            cached: 1,
            missing_local: 2,
            total_size: 11,
            remote_checked: true,
            on_remote: Some(1),
            unpushed: Some(1),
            remote_unknown: Some(1),
            files: vec![
                StatusFile {
                    path: "present.bin".into(),
                    hash,
                    size: 11,
                    cached: true,
                    remote: Some(RemoteState::Present),
                },
                StatusFile {
                    path: "absent.bin".into(),
                    hash,
                    size: status::SIZE_UNKNOWN,
                    cached: false,
                    remote: Some(RemoteState::Absent {
                        reason: RemoteAbsenceReason::NotListed,
                    }),
                },
                StatusFile {
                    path: "unknown.bin".into(),
                    hash,
                    size: status::SIZE_UNKNOWN,
                    cached: false,
                    remote: Some(RemoteState::Unknown {
                        cause: "remote unavailable".to_owned(),
                    }),
                },
            ],
        };

        assert_eq!(
            serde_json::to_value(status_json(&report)).unwrap(),
            json!({
                "tracked": 3,
                "unique_files": 3,
                "cached": 1,
                "missing_local": 2,
                "total_size": 11,
                "remote_checked": true,
                "on_remote": 1,
                "unpushed": 1,
                "remote_unknown": 1,
                "files": [
                    {
                        "path": "present.bin",
                        "hash": hash.to_string(),
                        "size": 11,
                        "cached": true,
                        "remote": { "state": "present" }
                    },
                    {
                        "path": "absent.bin",
                        "hash": hash.to_string(),
                        "size": -1,
                        "cached": false,
                        "remote": {
                            "state": "absent",
                            "reason": "not_listed"
                        }
                    },
                    {
                        "path": "unknown.bin",
                        "hash": hash.to_string(),
                        "size": -1,
                        "cached": false,
                        "remote": {
                            "state": "unknown",
                            "cause": "remote unavailable"
                        }
                    }
                ]
            })
        );
    }

    #[test]
    fn status_json_omits_remote_fields_when_unchecked() {
        let report = StatusReport {
            tracked: 1,
            unique_files: 1,
            cached: 1,
            missing_local: 0,
            total_size: 11,
            remote_checked: false,
            on_remote: None,
            unpushed: None,
            remote_unknown: None,
            files: vec![StatusFile {
                path: "local.bin".into(),
                hash: hash(),
                size: 11,
                cached: true,
                remote: None,
            }],
        };
        let payload = serde_json::to_value(status_json(&report)).unwrap();

        assert!(payload.get("on_remote").is_none());
        assert!(payload.get("unpushed").is_none());
        assert!(payload.get("remote_unknown").is_none());
        assert!(payload["files"][0].get("remote").is_none());
    }
}
