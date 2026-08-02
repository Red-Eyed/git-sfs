//! Terminal rendering for command outcomes.
//!
//! `dispatch` owns orchestration; this module owns the strings users see.
//! Keeping that split in the binary crate preserves `git-sfs-core`'s invariant
//! that it never prints and never learns about `--quiet`.

use serde::Serialize;

use git_sfs_core::exec::add::AddOutcome;
use git_sfs_core::exec::doctor::{DoctorReport, DoctorStatus};
use git_sfs_core::exec::import::ImportOutcome;
use git_sfs_core::exec::init::InitOutcome;
use git_sfs_core::exec::mv::MovedLink;
use git_sfs_core::exec::pull::PullOutcome;
use git_sfs_core::exec::push::PushOutcome;
use git_sfs_core::exec::remotes::RemoteEntry;
use git_sfs_core::exec::setup::SetupOutcome;
use git_sfs_core::exec::verify::{self, VerifyIssue, VerifyReport};
use git_sfs_core::{Error, Result};

/// How much non-essential output a command should emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderMode {
    Normal,
    Quiet,
}

impl RenderMode {
    pub(crate) fn from_quiet(quiet: bool) -> Self {
        if quiet { Self::Quiet } else { Self::Normal }
    }

    fn show_success(self) -> bool {
        self == Self::Normal
    }
}

pub(crate) fn add_outcome(outcome: &AddOutcome, mode: RenderMode) {
    if mode.show_success() {
        for line in add_success_lines(outcome) {
            println!("{line}");
        }
    }
    for line in unrepresentable_warnings(&outcome.unrepresentable) {
        eprintln!("{line}");
    }
}

pub(crate) fn moved_links(moved: &[MovedLink], mode: RenderMode) {
    if !mode.show_success() {
        return;
    }
    for link in moved {
        println!("{}", moved_link_line(link));
    }
}

pub(crate) fn import_outcome(outcome: &ImportOutcome, mode: RenderMode) {
    if mode.show_success() {
        for line in import_success_lines(outcome) {
            println!("{line}");
        }
    }
    for line in unrepresentable_warnings(&outcome.unrepresentable) {
        eprintln!("{line}");
    }
}

pub(crate) fn init_outcome(outcome: &InitOutcome, mode: RenderMode) {
    if !mode.show_success() {
        return;
    }
    println!("initialized git-sfs repository");
    println!("config: {}", outcome.config_path);
    println!("cache: {}", outcome.cache_root);
}

pub(crate) fn setup_outcome(outcome: &SetupOutcome, mode: RenderMode) {
    if mode.show_success() {
        println!("setup complete");
        println!("cache: {}", outcome.cache_root);
    }
}

pub(crate) fn push_outcome(outcome: &PushOutcome, mode: RenderMode) {
    for line in skipped_push_warning_lines(outcome) {
        eprintln!("{line}");
    }
    if mode.show_success() && !outcome.uploaded.is_empty() {
        println!(
            "push: uploaded {} file(s) to remote",
            outcome.uploaded.len()
        );
    }
}

pub(crate) fn pull_outcome(outcome: &PullOutcome, mode: RenderMode) {
    if mode.show_success() && !outcome.downloaded.is_empty() {
        println!(
            "pull: downloaded {} file(s) from remote",
            outcome.downloaded.len()
        );
    }
}

pub(crate) fn doctor_report(report: &DoctorReport) {
    for section in report.sections() {
        if let Some(title) = section.title() {
            println!("\n  [{title}]");
        }
        for check in section.checks() {
            println!("{}", doctor_check_line(check.label(), check.status()));
        }
    }
    println!();
    println!("{}", doctor_summary_line(report));
}

pub(crate) fn verify_success(report: &VerifyReport, mode: RenderMode) {
    verify_report(report);
    if mode.show_success() {
        println!("verify ok");
    }
}

pub(crate) fn verify_report(report: &VerifyReport) {
    let counts = report.counts();
    println!("tracked symlinks: {}", report.tracked_symlinks);
    for kind in verify::ISSUE_KINDS {
        println!("{}: {}", kind.plural(), counts.get(kind).unwrap_or(&0));
    }
    if report.orphan_count > 0 {
        println!("# {} orphaned cache object(s)", report.orphan_count);
    }
    if report.issues.is_empty() {
        return;
    }
    println!("details:");
    for issue in &report.issues {
        println!("{}", verify_issue_line(issue));
    }
}

pub(crate) fn remotes_text(entries: &[RemoteEntry]) {
    println!("remotes: {}", entries.len());
    for entry in entries {
        println!("{}", remote_line(entry));
    }
}

pub(crate) fn remotes_json(entries: &[RemoteEntry]) -> Result<()> {
    let payload = remotes_json_payload(entries);
    serde_json::to_writer_pretty(std::io::stdout(), &payload)
        .map_err(|err| Error::Unavailable(format!("could not write remotes JSON: {err}")))?;
    println!();
    Ok(())
}

fn remotes_json_payload(entries: &[RemoteEntry]) -> RemotesJson<'_> {
    RemotesJson { remotes: entries }
}

fn add_success_lines(outcome: &AddOutcome) -> Vec<String> {
    outcome
        .added
        .iter()
        .map(|file| format!("added {} -> {}", file.path, file.hash))
        .collect()
}

fn moved_link_line(link: &MovedLink) -> String {
    format!("moved {} -> {}", link.old_path, link.new_path)
}

fn import_success_lines(outcome: &ImportOutcome) -> Vec<String> {
    outcome
        .imported
        .iter()
        .map(|file| format!("imported {} -> {} -> {}", file.src, file.dst, file.hash))
        .collect()
}

fn unrepresentable_warnings(descriptions: &[String]) -> Vec<String> {
    descriptions
        .iter()
        .map(|description| {
            format!("git-sfs: warning: skipped {description} (not a valid UTF-8 path)")
        })
        .collect()
}

fn skipped_push_warning_lines(outcome: &PushOutcome) -> Vec<String> {
    if outcome.skipped.is_empty() {
        return Vec::new();
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

    let mut lines = vec![format!(
        "git-sfs: warning: push skipped {} missing cached object(s)",
        outcome.skipped.len()
    )];
    for (path, hash) in skipped_paths.iter().take(MAX_WARNINGS) {
        lines.push(format!("  {path} ({})", hash.short()));
    }
    if skipped_paths.len() > MAX_WARNINGS {
        lines.push(format!(
            "  ... {} more path(s) skipped",
            skipped_paths.len() - MAX_WARNINGS
        ));
    }
    lines.push("  run: git-sfs pull <path> to restore them".to_owned());
    lines
}

fn doctor_check_line(label: &str, status: &DoctorStatus) -> String {
    match status {
        DoctorStatus::Pass { detail } if detail.is_empty() => {
            format!("  {label:<24} ok", label = format!("{label}:"))
        }
        DoctorStatus::Pass { detail } => {
            format!("  {label:<24} ok  ({detail})", label = format!("{label}:"))
        }
        DoctorStatus::Fail { detail } => {
            format!("  {label:<24} FAIL: {detail}", label = format!("{label}:"))
        }
        DoctorStatus::Skip => {
            format!("  {label:<24} skip", label = format!("{label}:"))
        }
    }
}

fn doctor_summary_line(report: &DoctorReport) -> String {
    match (report.failed(), report.skipped()) {
        (0, 0) => format!("doctor: all {} checks passed", report.passed()),
        (0, skipped) => format!("doctor: {} passed, {skipped} skipped", report.passed()),
        (failed, skipped) => {
            format!(
                "doctor: {} passed, {failed} failed, {skipped} skipped",
                report.passed()
            )
        }
    }
}

fn verify_issue_line(issue: &VerifyIssue) -> String {
    let mut parts = vec![issue.kind.singular().to_owned()];
    if let Some(path) = &issue.path {
        parts.push(path.to_string());
    }
    if let Some(hash) = issue.hash {
        parts.push(hash.to_string());
    }
    let mut line = parts.join(": ");
    if let Some(detail) = &issue.detail {
        line.push_str(": ");
        line.push_str(detail);
    }
    line
}

fn remote_line(entry: &RemoteEntry) -> String {
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use git_sfs_core::domain::hash::Sha256;
    use git_sfs_core::exec::add::AddedFile;
    use git_sfs_core::exec::push::PushOutcome;
    use git_sfs_core::exec::remotes::RemoteEntry;
    use git_sfs_core::plan::SkippedObject;
    use serde_json::json;

    use super::*;

    fn hash() -> Sha256 {
        Sha256::parse("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .expect("valid hash")
    }

    #[test]
    fn quiet_mode_hides_success_chatter() {
        assert!(RenderMode::Normal.show_success());
        assert!(!RenderMode::Quiet.show_success());
    }

    #[test]
    fn add_success_lines_match_the_existing_human_shape() {
        let outcome = AddOutcome {
            added: vec![AddedFile {
                path: Utf8PathBuf::from("data/blob.bin"),
                hash: hash(),
            }],
            unrepresentable: Vec::new(),
        };

        assert_eq!(
            add_success_lines(&outcome),
            [
                "added data/blob.bin -> 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ]
        );
    }

    #[test]
    fn skipped_push_warning_is_capped_and_keeps_the_repair_hint() {
        let skipped = SkippedObject {
            hash: hash(),
            paths: (0..12)
                .map(|index| Utf8PathBuf::from(format!("data/{index}.bin")))
                .collect(),
        };
        let lines = skipped_push_warning_lines(&PushOutcome {
            uploaded: Vec::new(),
            skipped: vec![skipped],
        });

        assert_eq!(
            lines.first().map(String::as_str),
            Some("git-sfs: warning: push skipped 1 missing cached object(s)")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "  ... 2 more path(s) skipped")
        );
        assert_eq!(
            lines.last().map(String::as_str),
            Some("  run: git-sfs pull <path> to restore them")
        );
    }

    #[test]
    fn remotes_json_shape_matches_the_contract() {
        let entries = vec![
            RemoteEntry {
                name: "default".to_owned(),
                backend: "myremote".to_owned(),
                path: Some("datasets/project".to_owned()),
                config: Some("rclone.conf".to_owned()),
                default: true,
            },
            RemoteEntry {
                name: "archive".to_owned(),
                backend: "archive".to_owned(),
                path: None,
                config: None,
                default: false,
            },
        ];

        assert_eq!(
            serde_json::to_value(remotes_json_payload(&entries)).unwrap(),
            json!({
                "remotes": [
                    {
                        "name": "default",
                        "backend": "myremote",
                        "path": "datasets/project",
                        "config": "rclone.conf",
                        "default": true
                    },
                    {
                        "name": "archive",
                        "backend": "archive",
                        "default": false
                    }
                ]
            })
        );
    }
}
