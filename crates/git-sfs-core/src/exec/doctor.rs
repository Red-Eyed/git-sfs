//! Structured `git-sfs doctor` output.
//!
//! `doctor` is mostly diagnostic shell work: inspect the current repository,
//! cache, configured rclone binary, and remotes, then print a readable report.
//! This module owns the report shape and deterministic helpers; the binary
//! crate owns terminal output and process-environment probing.

use crate::domain::{Config, DEFAULT_REMOTE_NAME};

/// One doctor run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    sections: Vec<DoctorSection>,
}

impl DoctorReport {
    /// An empty report.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sections: vec![DoctorSection::new(None)],
        }
    }

    /// All sections, in display order.
    #[must_use]
    pub fn sections(&self) -> &[DoctorSection] {
        &self.sections
    }

    /// Start a titled section.
    pub fn section(&mut self, title: impl Into<String>) {
        self.sections.push(DoctorSection::new(Some(title.into())));
    }

    /// Record a passed check.
    pub fn pass(&mut self, label: impl Into<String>, detail: impl Into<String>) {
        self.current().checks.push(DoctorCheck::pass(label, detail));
    }

    /// Record a failed check.
    pub fn fail(&mut self, label: impl Into<String>, detail: impl Into<String>) {
        self.current().checks.push(DoctorCheck::fail(label, detail));
    }

    /// Record a skipped check.
    pub fn skip(&mut self, label: impl Into<String>) {
        self.current().checks.push(DoctorCheck::skip(label));
    }

    /// Record several skipped checks.
    pub fn skip_all(&mut self, labels: &[&str]) {
        for label in labels {
            self.skip(*label);
        }
    }

    /// Number of passed checks.
    #[must_use]
    pub fn passed(&self) -> usize {
        self.count(DoctorStatus::is_pass)
    }

    /// Number of failed checks.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.count(DoctorStatus::is_fail)
    }

    /// Number of skipped checks.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.count(DoctorStatus::is_skip)
    }

    /// Whether any check failed.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.failed() > 0
    }

    fn current(&mut self) -> &mut DoctorSection {
        self.sections
            .last_mut()
            .expect("DoctorReport always has an initial section")
    }

    fn count(&self, predicate: fn(&DoctorStatus) -> bool) -> usize {
        self.sections
            .iter()
            .flat_map(|section| section.checks.iter())
            .filter(|check| predicate(&check.status))
            .count()
    }
}

impl Default for DoctorReport {
    fn default() -> Self {
        Self::new()
    }
}

/// A display section in a doctor report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorSection {
    title: Option<String>,
    checks: Vec<DoctorCheck>,
}

impl DoctorSection {
    fn new(title: Option<String>) -> Self {
        Self {
            title,
            checks: Vec::new(),
        }
    }

    /// Optional section title.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Checks in this section.
    #[must_use]
    pub fn checks(&self) -> &[DoctorCheck] {
        &self.checks
    }
}

/// One doctor check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    label: String,
    status: DoctorStatus,
}

impl DoctorCheck {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: DoctorStatus::Pass {
                detail: detail.into(),
            },
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: DoctorStatus::Fail {
                detail: detail.into(),
            },
        }
    }

    fn skip(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: DoctorStatus::Skip,
        }
    }

    /// Check label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Check status.
    #[must_use]
    pub fn status(&self) -> &DoctorStatus {
        &self.status
    }
}

/// Result of one doctor check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoctorStatus {
    /// Check passed, optionally with detail.
    Pass {
        /// Human-readable detail.
        detail: String,
    },
    /// Check failed.
    Fail {
        /// Human-readable reason.
        detail: String,
    },
    /// Check was skipped because an earlier prerequisite failed.
    Skip,
}

impl DoctorStatus {
    fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }

    fn is_fail(&self) -> bool {
        matches!(self, Self::Fail { .. })
    }

    fn is_skip(&self) -> bool {
        matches!(self, Self::Skip)
    }
}

/// Remote names `doctor` should check, with `default` first when all remotes
/// are requested.
#[must_use]
pub fn remote_names(config: &Config, filter: Option<&str>) -> Vec<String> {
    if let Some(name) = filter {
        return vec![name.to_owned()];
    }
    let mut names = config
        .remotes
        .keys()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    names.sort_by(|left, right| match (left.as_str(), right.as_str()) {
        (DEFAULT_REMOTE_NAME, DEFAULT_REMOTE_NAME) => std::cmp::Ordering::Equal,
        (DEFAULT_REMOTE_NAME, _) => std::cmp::Ordering::Less,
        (_, DEFAULT_REMOTE_NAME) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    });
    names
}

#[cfg(test)]
mod tests {
    use crate::domain::{RemoteConfig, RemoteName, Settings};

    use super::*;

    fn config_with_remotes(names: &[&str]) -> Config {
        let remotes = names
            .iter()
            .map(|name| {
                (
                    RemoteName::parse(*name).unwrap(),
                    RemoteConfig {
                        backend: name.to_string(),
                        path: None,
                        rclone_config_path: None,
                    },
                )
            })
            .collect();
        Config {
            remotes,
            settings: Settings {
                algorithm: "sha256".to_owned(),
                jobs: 0,
                retry_max: None,
                min_rclone_version: None,
                min_git_sfs_version: None,
            },
        }
    }

    #[test]
    fn report_counts_statuses_across_sections() {
        let mut report = DoctorReport::new();
        report.pass("repo", "/repo");
        report.skip("cache");
        report.section("remote: default");
        report.fail("remote path", "missing");

        assert_eq!(report.passed(), 1);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.skipped(), 1);
        assert!(report.has_failures());
    }

    #[test]
    fn remote_names_put_default_first_then_sort_the_rest() {
        let config = config_with_remotes(&["zeta", "default", "alpha"]);

        assert_eq!(remote_names(&config, None), ["default", "alpha", "zeta"]);
    }

    #[test]
    fn remote_names_honor_a_filter_even_when_missing_from_config() {
        let config = config_with_remotes(&["default"]);

        assert_eq!(remote_names(&config, Some("backup")), ["backup"]);
    }
}
