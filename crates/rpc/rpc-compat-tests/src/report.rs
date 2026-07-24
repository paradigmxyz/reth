//! Reading compatibility reports for CI summaries.

use eyre::{Context, Result};
use serde::Deserialize;
use std::{fs, path::Path};

/// One result that should fail the compatibility run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexpectedResult {
    /// Canonical test ID.
    pub id: String,
    /// Why the result is unexpected.
    pub kind: UnexpectedKind,
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
    let contents = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read compatibility report {}", path.display()))?;
    parse(&contents)
        .wrap_err_with(|| format!("failed to parse compatibility report {}", path.display()))
}

fn parse(contents: &str) -> Result<Vec<UnexpectedResult>> {
    let report: Report = serde_json::from_str(contents)?;
    Ok(report
        .results
        .into_iter()
        .filter_map(|result| {
            let kind = match result.outcome.as_str() {
                "fail" => UnexpectedKind::Failure,
                "xpass" => UnexpectedKind::UnexpectedPass,
                _ => return None,
            };
            Some(UnexpectedResult { id: result.id, kind })
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct Report {
    results: Vec<ReportResult>,
}

#[derive(Debug, Deserialize)]
struct ReportResult {
    id: String,
    outcome: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_unexpected_results() {
        let results = parse(
            r#"{
                "results": [
                    {"id": "eth_ok/test", "outcome": "pass"},
                    {"id": "eth_bad/test", "outcome": "fail"},
                    {"id": "eth_fixed/test", "outcome": "xpass"},
                    {"id": "eth_known/test", "outcome": "xfail"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            results,
            vec![
                UnexpectedResult { id: "eth_bad/test".to_string(), kind: UnexpectedKind::Failure },
                UnexpectedResult {
                    id: "eth_fixed/test".to_string(),
                    kind: UnexpectedKind::UnexpectedPass,
                },
            ]
        );
    }
}
