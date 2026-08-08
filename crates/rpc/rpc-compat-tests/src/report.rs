//! Reading compatibility reports for CI summaries.

use eyre::{eyre, Context, Result};
use serde::Deserialize;
use std::{fs, path::Path};

/// Aggregate results from one compatibility report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSummary {
    /// Fixture revision used for the run.
    pub fixture_revision: String,
    /// Total run duration in milliseconds.
    pub duration_ms: u128,
    /// Number of selected tests.
    pub selected: usize,
    /// Number of tests that were executed.
    pub executed: usize,
    /// Number of ordinary passing tests.
    pub passed: usize,
    /// Canonical IDs of ordinary passing tests.
    pub passed_tests: Vec<String>,
    /// Number of unexpected failing tests.
    pub failed: usize,
    /// Number of expected failures.
    pub expected_failures: usize,
    /// Number of expected failures that unexpectedly passed.
    pub unexpected_passes: usize,
    /// Number of ignored tests.
    pub ignored: usize,
    /// Number of skipped tests.
    pub skipped: usize,
    /// Results that should fail CI.
    pub unexpected: Vec<UnexpectedResult>,
}

/// One result that should fail the compatibility run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexpectedResult {
    /// Canonical test ID.
    pub id: String,
    /// Why the result is unexpected.
    pub kind: UnexpectedKind,
    /// Full mismatch detail, when available.
    pub detail: Option<String>,
}

/// Classification of an unexpected result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnexpectedKind {
    /// A normal test failed.
    Failure,
    /// An expected failure passed.
    UnexpectedPass,
}

/// Reads unexpected results from a JSON report.
pub fn unexpected_results(path: &Path) -> Result<Vec<UnexpectedResult>> {
    Ok(report_summary(path)?.unexpected)
}

/// Reads aggregate and unexpected results from a JSON report.
pub fn report_summary(path: &Path) -> Result<ReportSummary> {
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read compatibility report {}", path.display()))?;
    summarize(&contents)
        .wrap_err_with(|| format!("failed to parse compatibility report {}", path.display()))
}

fn summarize(contents: &str) -> Result<ReportSummary> {
    let report: Report = serde_json::from_str(contents)?;
    let mut summary = ReportSummary {
        fixture_revision: report.fixture_revision,
        duration_ms: report.duration_ms,
        selected: report.results.len(),
        executed: 0,
        passed: 0,
        passed_tests: Vec::new(),
        failed: 0,
        expected_failures: 0,
        unexpected_passes: 0,
        ignored: 0,
        skipped: 0,
        unexpected: Vec::new(),
    };
    for result in report.results {
        match result.outcome.as_str() {
            "pass" => {
                summary.passed += 1;
                summary.passed_tests.push(result.id);
            }
            "fail" => {
                summary.failed += 1;
                summary.unexpected.push(UnexpectedResult {
                    id: result.id,
                    kind: UnexpectedKind::Failure,
                    detail: result.detail,
                });
            }
            "xfail" => summary.expected_failures += 1,
            "xpass" => {
                summary.unexpected_passes += 1;
                summary.unexpected.push(UnexpectedResult {
                    id: result.id,
                    kind: UnexpectedKind::UnexpectedPass,
                    detail: result.detail,
                });
            }
            "ignored-pass" | "ignored-fail" => summary.ignored += 1,
            "skip" => summary.skipped += 1,
            outcome => return Err(eyre!("unknown compatibility outcome `{outcome}`")),
        }
    }
    summary.executed = summary.selected - summary.skipped;
    Ok(summary)
}

#[derive(Debug, Deserialize)]
struct Report {
    fixture_revision: String,
    duration_ms: u128,
    results: Vec<ReportResult>,
}

#[derive(Debug, Deserialize)]
struct ReportResult {
    id: String,
    outcome: String,
    detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_results_and_selects_unexpected() {
        let summary = summarize(
            r#"{
                "fixture_revision": "abc123",
                "duration_ms": 3745,
                "results": [
                    {"id": "eth_ok/test", "outcome": "pass", "detail": null},
                    {"id": "eth_bad/test", "outcome": "fail", "detail": "a clean diff"},
                    {"id": "eth_fixed/test", "outcome": "xpass", "detail": null},
                    {"id": "eth_known/test", "outcome": "xfail", "detail": "known"},
                    {"id": "eth_ignored/test", "outcome": "ignored-fail", "detail": "ignored"},
                    {"id": "eth_skip/test", "outcome": "skip", "detail": null}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(summary.fixture_revision, "abc123");
        assert_eq!(summary.duration_ms, 3745);
        assert_eq!(summary.selected, 6);
        assert_eq!(summary.executed, 5);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.passed_tests, ["eth_ok/test"]);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.expected_failures, 1);
        assert_eq!(summary.unexpected_passes, 1);
        assert_eq!(summary.ignored, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(
            summary.unexpected,
            vec![
                UnexpectedResult {
                    id: "eth_bad/test".to_string(),
                    kind: UnexpectedKind::Failure,
                    detail: Some("a clean diff".to_string()),
                },
                UnexpectedResult {
                    id: "eth_fixed/test".to_string(),
                    kind: UnexpectedKind::UnexpectedPass,
                    detail: None,
                },
            ]
        );
    }
}
