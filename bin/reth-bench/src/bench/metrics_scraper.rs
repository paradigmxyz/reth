//! Prometheus metrics scraper for reth-bench.
//!
//! Scrapes a node's Prometheus metrics endpoint after each block to record
//! all metric values with block-level granularity.

use eyre::Context;
use reqwest::Client;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
    time::Duration,
};
use tracing::info;

/// Suffix for the metrics CSV output file.
pub(crate) const METRICS_OUTPUT_SUFFIX: &str = "metrics.csv";

/// Scrapes a Prometheus metrics endpoint after each block to collect
/// all metric values.
pub(crate) struct MetricsScraper {
    /// The full URL of the Prometheus metrics endpoint.
    url: String,
    /// Reusable HTTP client.
    client: Client,
    /// Collected metrics snapshots, one per block.
    rows: Vec<MetricsSnapshot>,
}

/// A single snapshot of all scraped prometheus metrics for one block.
struct MetricsSnapshot {
    /// The block number.
    block_number: u64,
    /// All metric name→value pairs from this scrape.
    values: BTreeMap<String, f64>,
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

    /// Scrapes the metrics endpoint and records all values for the given block.
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

        let values = parse_all_metrics(&text);
        self.rows.push(MetricsSnapshot { block_number, values });
        Ok(())
    }

    /// Writes collected metrics to a CSV file in the output directory.
    ///
    /// The CSV has a dynamic set of columns derived from the union of all metric
    /// names seen across all scraped blocks. Missing values are left empty.
    pub(crate) fn write_csv(&self, output_dir: &Path) -> eyre::Result<()> {
        let path = output_dir.join(METRICS_OUTPUT_SUFFIX);
        info!(target: "reth-bench", "Writing scraped metrics to file: {:?}", path);

        // Collect the union of all metric names across all snapshots.
        let all_names: BTreeSet<&str> =
            self.rows.iter().flat_map(|s| s.values.keys().map(|k| k.as_str())).collect();
        let columns: Vec<&str> = all_names.into_iter().collect();

        let mut writer = std::io::BufWriter::new(std::fs::File::create(&path)?);

        // Write header.
        write!(writer, "block_number")?;
        for col in &columns {
            write!(writer, ",{col}")?;
        }
        writeln!(writer)?;

        // Write rows.
        for snapshot in &self.rows {
            write!(writer, "{}", snapshot.block_number)?;
            for col in &columns {
                if let Some(val) = snapshot.values.get(*col) {
                    write!(writer, ",{val}")?;
                } else {
                    write!(writer, ",")?;
                }
            }
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(())
    }
}

/// Parses all metric values from Prometheus exposition-format text.
///
/// For metrics with labels (e.g. `metric{label="value"} 1.0`), the label set is
/// appended to the name so each unique label combination gets its own column in
/// the output CSV.
///
/// When multiple samples share the same fully-qualified name (name + labels),
/// the last value wins.
fn parse_all_metrics(text: &str) -> BTreeMap<String, f64> {
    let mut result = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Prometheus exposition format: `name{labels} value [timestamp]`
        // or: `name value [timestamp]`
        let (name_and_labels, rest) = if let Some(brace_start) = line.find('{') {
            if let Some(brace_end) = line[brace_start..].find('}') {
                let end = brace_start + brace_end + 1;
                (&line[..end], line[end..].trim())
            } else {
                continue;
            }
        } else if let Some(space) = line.find(' ') {
            (&line[..space], line[space..].trim())
        } else {
            continue;
        };

        // The value is the first whitespace-separated token in the rest.
        if let Some(value_str) = rest.split_whitespace().next() &&
            let Ok(v) = value_str.parse::<f64>()
        {
            result.insert(name_and_labels.to_string(), v);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_all_metrics_simple() {
        let text = r#"# HELP sync_execution_execution_duration Duration of execution
# TYPE sync_execution_execution_duration gauge
sync_execution_execution_duration 0.123456
some_counter 42
"#;
        let metrics = parse_all_metrics(text);
        assert_eq!(metrics.get("sync_execution_execution_duration"), Some(&0.123456));
        assert_eq!(metrics.get("some_counter"), Some(&42.0));
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn test_parse_all_metrics_with_labels() {
        let text = r#"http_requests_total{method="GET",status="200"} 100
http_requests_total{method="POST",status="200"} 50
"#;
        let metrics = parse_all_metrics(text);
        assert_eq!(metrics.get(r#"http_requests_total{method="GET",status="200"}"#), Some(&100.0));
        assert_eq!(metrics.get(r#"http_requests_total{method="POST",status="200"}"#), Some(&50.0));
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn test_parse_all_metrics_skips_comments_and_empty() {
        let text = r#"
# HELP my_metric A metric
# TYPE my_metric gauge

my_metric 1.5
"#;
        let metrics = parse_all_metrics(text);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics.get("my_metric"), Some(&1.5));
    }

    #[test]
    fn test_parse_all_metrics_with_timestamp() {
        let text = "my_metric 1.5 1234567890\n";
        let metrics = parse_all_metrics(text);
        assert_eq!(metrics.get("my_metric"), Some(&1.5));
    }

    #[test]
    fn test_parse_all_metrics_last_value_wins() {
        let text = "my_metric 1.0\nmy_metric 2.0\n";
        let metrics = parse_all_metrics(text);
        assert_eq!(metrics.get("my_metric"), Some(&2.0));
    }

    #[test]
    fn test_write_csv_dynamic_columns() {
        let scraper = MetricsScraper {
            url: String::new(),
            client: Client::new(),
            rows: vec![
                MetricsSnapshot {
                    block_number: 1,
                    values: BTreeMap::from([
                        ("metric_a".to_string(), 1.0),
                        ("metric_b".to_string(), 2.0),
                    ]),
                },
                MetricsSnapshot {
                    block_number: 2,
                    values: BTreeMap::from([
                        ("metric_a".to_string(), 3.0),
                        ("metric_c".to_string(), 4.0),
                    ]),
                },
            ],
        };

        let dir = tempfile::tempdir().unwrap();
        scraper.write_csv(dir.path()).unwrap();

        let csv = std::fs::read_to_string(dir.path().join("metrics.csv")).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "block_number,metric_a,metric_b,metric_c");
        assert_eq!(lines[1], "1,1,2,");
        assert_eq!(lines[2], "2,3,,4");
    }
}
