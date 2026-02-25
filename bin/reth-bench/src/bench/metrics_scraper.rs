//! Prometheus metrics scraper for reth-bench.
//!
//! Scrapes a node's Prometheus metrics endpoint after each block to record
//! execution and state root durations with block-level granularity.

use csv::Writer;
use eyre::Context;
use reqwest::Client;
use serde::Serialize;
use std::{path::Path, time::Duration};
use tracing::info;

/// Suffix for the metrics CSV output file.
pub(crate) const METRICS_OUTPUT_SUFFIX: &str = "metrics.csv";

/// A single row of scraped prometheus metrics for one block.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MetricsRow {
    /// The block number.
    pub(crate) block_number: u64,
    /// EVM execution duration in seconds (from `sync_execution_execution_duration` gauge).
    pub(crate) execution_duration_secs: Option<f64>,
    /// State root computation duration in seconds (from
    /// `sync_block_validation_state_root_duration` gauge).
    pub(crate) state_root_duration_secs: Option<f64>,
}

/// Scrapes a Prometheus metrics endpoint after each block to collect
/// execution and state root durations.
pub(crate) struct MetricsScraper {
    /// The full URL of the Prometheus metrics endpoint.
    url: String,
    /// Reusable HTTP client.
    client: Client,
    /// Collected metrics rows, one per block.
    rows: Vec<MetricsRow>,
}

impl MetricsScraper {
    /// Creates a new scraper if a URL is provided.
    pub(crate) fn maybe_new(url: Option<String>) -> Option<Self> {
        url.map(|url| {
            info!(target: "reth-bench", %url, "Prometheus metrics scraping enabled");
            let client = Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("failed to build reqwest client");
            Self { url, client, rows: Vec::new() }
        })
    }

    /// Scrapes the metrics endpoint and records values for the given block.
    pub(crate) async fn scrape_after_block(&mut self, block_number: u64) -> eyre::Result<()> {
        let text = self
            .client
            .get(&self.url)
            .send()
            .await
            .wrap_err("failed to fetch metrics endpoint")?
            .error_for_status()
            .wrap_err("metrics endpoint returned error status")?
            .text()
            .await
            .wrap_err("failed to read metrics response body")?;

        let execution = parse_gauge(&text, "sync_execution_execution_duration");
        let state_root = parse_gauge(&text, "sync_block_validation_state_root_duration");

        self.rows.push(MetricsRow {
            block_number,
            execution_duration_secs: execution,
            state_root_duration_secs: state_root,
        });
        Ok(())
    }

    /// Writes collected metrics to a CSV file in the output directory.
    pub(crate) fn write_csv(&self, output_dir: &Path) -> eyre::Result<()> {
        let path = output_dir.join(METRICS_OUTPUT_SUFFIX);
        info!(target: "reth-bench", "Writing scraped metrics to file: {:?}", path);
        let mut writer = Writer::from_path(&path)?;
        for row in &self.rows {
            writer.serialize(row)?;
        }
        writer.flush()?;
        Ok(())
    }
}

/// Parses a Prometheus gauge value from exposition-format text.
///
/// Searches for lines starting with `name` followed by either a space or `{`
/// (for labeled metrics), then parses the numeric value. Returns the last
/// matching sample to handle metrics emitted with multiple label sets.
fn parse_gauge(text: &str, name: &str) -> Option<f64> {
    let mut result = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if !line.starts_with(name) {
            continue;
        }

        // Ensure we match the full metric name, not a prefix of another metric.
        let rest = &line[name.len()..];
        if !rest.starts_with(' ') && !rest.starts_with('{') {
            continue;
        }

        // Format: `metric_name{labels} value [timestamp]` or `metric_name value [timestamp]`
        // Value is always the second whitespace-separated token.
        let mut parts = line.split_whitespace();
        if let Some(value_str) = parts.nth(1) &&
            let Ok(v) = value_str.parse::<f64>()
        {
            result = Some(v);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gauge_simple() {
        let text = r#"# HELP sync_execution_execution_duration Duration of execution
# TYPE sync_execution_execution_duration gauge
sync_execution_execution_duration 0.123456
"#;
        assert_eq!(parse_gauge(text, "sync_execution_execution_duration"), Some(0.123456));
    }

    #[test]
    fn test_parse_gauge_missing() {
        let text = "some_other_metric 1.0\n";
        assert_eq!(parse_gauge(text, "sync_execution_execution_duration"), None);
    }

    #[test]
    fn test_parse_gauge_with_labels() {
        let text = "sync_block_validation_state_root_duration{instance=\"node1\"} 0.5\n";
        assert_eq!(parse_gauge(text, "sync_block_validation_state_root_duration"), Some(0.5));
    }

    #[test]
    fn test_parse_gauge_prefix_no_false_match() {
        let text =
            "sync_execution_execution_duration_total 99.0\nsync_execution_execution_duration 0.5\n";
        assert_eq!(parse_gauge(text, "sync_execution_execution_duration"), Some(0.5));
    }
}
