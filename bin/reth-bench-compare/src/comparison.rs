//! Results comparison and report generation.

use crate::{cli::Args, git::sanitize_git_ref};
use chrono::{DateTime, Utc};
use csv::Reader;
use eyre::{eyre, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use tracing::{info, warn};

/// Manages comparison between baseline and feature reference results
pub struct ComparisonGenerator {
    output_dir: PathBuf,
    timestamp: String,
    baseline_ref_name: String,
    feature_ref_name: String,
    baseline_results: Option<BenchmarkResults>,
    feature_results: Option<BenchmarkResults>,
}

/// Represents the results from a single benchmark run
#[derive(Debug, Clone)]
pub struct BenchmarkResults {
    pub ref_name: String,
    pub combined_latency_data: Vec<CombinedLatencyRow>,
    pub summary: BenchmarkSummary,
}

/// Combined latency CSV row structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CombinedLatencyRow {
    pub block_number: u64,
    pub gas_used: u64,
    pub new_payload_latency: u128,
    pub fcu_latency: u128,
    pub total_latency: u128,
}

/// Total gas CSV row structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TotalGasRow {
    pub block_number: u64,
    pub gas_used: u64,
    pub time: u128,
}

/// Summary statistics for a benchmark run
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkSummary {
    pub total_blocks: u64,
    pub total_gas_used: u64,
    pub total_duration_ms: u128,
    pub avg_new_payload_latency_ms: f64,
    pub avg_fcu_latency_ms: f64,
    pub avg_total_latency_ms: f64,
    pub gas_per_second: f64,
    pub blocks_per_second: f64,
}

/// Comparison report between two benchmark runs
#[derive(Debug, Serialize)]
pub struct ComparisonReport {
    pub timestamp: String,
    pub baseline: RefInfo,
    pub feature: RefInfo,
    pub comparison_summary: ComparisonSummary,
    pub per_block_comparisons: Vec<BlockComparison>,
}

/// Information about a reference in the comparison
#[derive(Debug, Serialize)]
pub struct RefInfo {
    pub ref_name: String,
    pub summary: BenchmarkSummary,
}

/// Summary of the comparison between references
#[derive(Debug, Serialize)]
pub struct ComparisonSummary {
    pub new_payload_latency_change_percent: f64,
    pub fcu_latency_change_percent: f64,
    pub total_latency_change_percent: f64,
    pub gas_per_second_change_percent: f64,
    pub blocks_per_second_change_percent: f64,
}

/// Per-block comparison data
#[derive(Debug, Serialize)]
pub struct BlockComparison {
    pub block_number: u64,
    pub baseline_new_payload_latency: u128,
    pub feature_new_payload_latency: u128,
    pub new_payload_latency_change_percent: f64,
    pub baseline_total_latency: u128,
    pub feature_total_latency: u128,
    pub total_latency_change_percent: f64,
}

impl ComparisonGenerator {
    /// Create a new comparison generator
    pub fn new(args: &Args) -> Self {
        let now: DateTime<Utc> = Utc::now();
        let timestamp = now.format("%Y%m%d_%H%M%S").to_string();

        Self {
            output_dir: args.output_dir_path(),
            timestamp,
            baseline_ref_name: args.baseline_ref.clone(),
            feature_ref_name: args.feature_ref.clone(),
            baseline_results: None,
            feature_results: None,
        }
    }

    /// Get the output directory for a specific reference
    pub fn get_ref_output_dir(&self, ref_type: &str) -> PathBuf {
        // Use the actual git reference name, sanitized for filesystem
        let ref_name = match ref_type {
            "baseline" => &self.baseline_ref_name,
            "feature" => &self.feature_ref_name,
            _ => ref_type, // fallback to the provided string
        };

        self.output_dir.join("results").join(&self.timestamp).join(sanitize_git_ref(ref_name))
    }

    /// Get the main output directory for this comparison run
    pub fn get_output_dir(&self) -> PathBuf {
        self.output_dir.join("results").join(&self.timestamp)
    }

    /// Add benchmark results for a reference
    pub fn add_ref_results(&mut self, ref_type: &str, output_path: &Path) -> Result<()> {
        let ref_name = match ref_type {
            "baseline" => &self.baseline_ref_name,
            "feature" => &self.feature_ref_name,
            _ => return Err(eyre!("Unknown reference type: {}", ref_type)),
        };

        let results = self.load_benchmark_results(ref_name, output_path)?;

        match ref_type {
            "baseline" => self.baseline_results = Some(results),
            "feature" => self.feature_results = Some(results),
            _ => return Err(eyre!("Unknown reference type: {}", ref_type)),
        }

        info!("Loaded benchmark results for {} reference", ref_type);

        Ok(())
    }

    /// Generate the final comparison report
    pub async fn generate_comparison_report(&self) -> Result<()> {
        info!("Generating comparison report...");

        let baseline =
            self.baseline_results.as_ref().ok_or_else(|| eyre!("Baseline results not loaded"))?;

        let feature =
            self.feature_results.as_ref().ok_or_else(|| eyre!("Feature results not loaded"))?;

        // Generate comparison
        let comparison_summary =
            self.calculate_comparison_summary(&baseline.summary, &feature.summary)?;
        let per_block_comparisons = self.calculate_per_block_comparisons(baseline, feature)?;

        let report = ComparisonReport {
            timestamp: self.timestamp.clone(),
            baseline: RefInfo {
                ref_name: baseline.ref_name.clone(),
                summary: baseline.summary.clone(),
            },
            feature: RefInfo {
                ref_name: feature.ref_name.clone(),
                summary: feature.summary.clone(),
            },
            comparison_summary,
            per_block_comparisons,
        };

        // Write reports
        self.write_comparison_reports(&report).await?;

        // Print summary to console
        self.print_comparison_summary(&report);

        Ok(())
    }

    /// Load benchmark results from CSV files
    fn load_benchmark_results(
        &self,
        ref_name: &str,
        output_path: &Path,
    ) -> Result<BenchmarkResults> {
        let combined_latency_path = output_path.join("combined_latency.csv");
        let total_gas_path = output_path.join("total_gas.csv");

        let combined_latency_data = self.load_combined_latency_csv(&combined_latency_path)?;
        let total_gas_data = self.load_total_gas_csv(&total_gas_path)?;

        let summary = self.calculate_summary(&combined_latency_data, &total_gas_data)?;

        Ok(BenchmarkResults { ref_name: ref_name.to_string(), combined_latency_data, summary })
    }

    /// Load combined latency CSV data
    fn load_combined_latency_csv(&self, path: &Path) -> Result<Vec<CombinedLatencyRow>> {
        let mut reader = Reader::from_path(path)
            .wrap_err_with(|| format!("Failed to open combined latency CSV: {path:?}"))?;

        let mut rows = Vec::new();
        for result in reader.deserialize() {
            let row: CombinedLatencyRow = result
                .wrap_err_with(|| format!("Failed to parse combined latency row in {path:?}"))?;
            rows.push(row);
        }

        if rows.is_empty() {
            return Err(eyre!("No data found in combined latency CSV: {:?}", path));
        }

        Ok(rows)
    }

    /// Load total gas CSV data
    fn load_total_gas_csv(&self, path: &Path) -> Result<Vec<TotalGasRow>> {
        let mut reader = Reader::from_path(path)
            .wrap_err_with(|| format!("Failed to open total gas CSV: {path:?}"))?;

        let mut rows = Vec::new();
        for result in reader.deserialize() {
            let row: TotalGasRow =
                result.wrap_err_with(|| format!("Failed to parse total gas row in {path:?}"))?;
            rows.push(row);
        }

        if rows.is_empty() {
            return Err(eyre!("No data found in total gas CSV: {:?}", path));
        }

        Ok(rows)
    }

    /// Calculate summary statistics for a benchmark run
    fn calculate_summary(
        &self,
        combined_data: &[CombinedLatencyRow],
        total_gas_data: &[TotalGasRow],
    ) -> Result<BenchmarkSummary> {
        if combined_data.is_empty() || total_gas_data.is_empty() {
            return Err(eyre!("Cannot calculate summary for empty data"));
        }

        let total_blocks = combined_data.len() as u64;
        let total_gas_used: u64 = combined_data.iter().map(|r| r.gas_used).sum();

        let total_duration_ms = total_gas_data.last().unwrap().time / 1000; // Convert microseconds to milliseconds

        let avg_new_payload_latency_ms: f64 =
            combined_data.iter().map(|r| r.new_payload_latency as f64 / 1000.0).sum::<f64>() /
                total_blocks as f64;

        let avg_fcu_latency_ms: f64 =
            combined_data.iter().map(|r| r.fcu_latency as f64 / 1000.0).sum::<f64>() /
                total_blocks as f64;

        let avg_total_latency_ms: f64 =
            combined_data.iter().map(|r| r.total_latency as f64 / 1000.0).sum::<f64>() /
                total_blocks as f64;

        let total_duration_seconds = total_duration_ms as f64 / 1000.0;
        let gas_per_second = if total_duration_seconds > 0.0 {
            total_gas_used as f64 / total_duration_seconds
        } else {
            0.0
        };

        let blocks_per_second = if total_duration_seconds > 0.0 {
            total_blocks as f64 / total_duration_seconds
        } else {
            0.0
        };

        Ok(BenchmarkSummary {
            total_blocks,
            total_gas_used,
            total_duration_ms,
            avg_new_payload_latency_ms,
            avg_fcu_latency_ms,
            avg_total_latency_ms,
            gas_per_second,
            blocks_per_second,
        })
    }

    /// Calculate comparison summary between baseline and feature
    fn calculate_comparison_summary(
        &self,
        baseline: &BenchmarkSummary,
        feature: &BenchmarkSummary,
    ) -> Result<ComparisonSummary> {
        let calc_percent_change = |baseline: f64, feature: f64| -> f64 {
            if baseline != 0.0 {
                ((feature - baseline) / baseline) * 100.0
            } else {
                0.0
            }
        };

        Ok(ComparisonSummary {
            new_payload_latency_change_percent: calc_percent_change(
                baseline.avg_new_payload_latency_ms,
                feature.avg_new_payload_latency_ms,
            ),
            fcu_latency_change_percent: calc_percent_change(
                baseline.avg_fcu_latency_ms,
                feature.avg_fcu_latency_ms,
            ),
            total_latency_change_percent: calc_percent_change(
                baseline.avg_total_latency_ms,
                feature.avg_total_latency_ms,
            ),
            gas_per_second_change_percent: calc_percent_change(
                baseline.gas_per_second,
                feature.gas_per_second,
            ),
            blocks_per_second_change_percent: calc_percent_change(
                baseline.blocks_per_second,
                feature.blocks_per_second,
            ),
        })
    }

    /// Calculate per-block comparisons
    fn calculate_per_block_comparisons(
        &self,
        baseline: &BenchmarkResults,
        feature: &BenchmarkResults,
    ) -> Result<Vec<BlockComparison>> {
        let mut baseline_map: HashMap<u64, &CombinedLatencyRow> = HashMap::new();
        for row in &baseline.combined_latency_data {
            baseline_map.insert(row.block_number, row);
        }

        let mut comparisons = Vec::new();
        for feature_row in &feature.combined_latency_data {
            if let Some(baseline_row) = baseline_map.get(&feature_row.block_number) {
                let calc_percent_change = |baseline: u128, feature: u128| -> f64 {
                    if baseline != 0 {
                        ((feature as f64 - baseline as f64) / baseline as f64) * 100.0
                    } else {
                        0.0
                    }
                };

                let comparison = BlockComparison {
                    block_number: feature_row.block_number,
                    baseline_new_payload_latency: baseline_row.new_payload_latency,
                    feature_new_payload_latency: feature_row.new_payload_latency,
                    new_payload_latency_change_percent: calc_percent_change(
                        baseline_row.new_payload_latency,
                        feature_row.new_payload_latency,
                    ),
                    baseline_total_latency: baseline_row.total_latency,
                    feature_total_latency: feature_row.total_latency,
                    total_latency_change_percent: calc_percent_change(
                        baseline_row.total_latency,
                        feature_row.total_latency,
                    ),
                };
                comparisons.push(comparison);
            } else {
                warn!("Block {} not found in baseline data", feature_row.block_number);
            }
        }

        Ok(comparisons)
    }

    /// Write comparison reports to files
    async fn write_comparison_reports(&self, report: &ComparisonReport) -> Result<()> {
        let report_dir = self.output_dir.join("results").join(&self.timestamp);
        fs::create_dir_all(&report_dir)
            .wrap_err_with(|| format!("Failed to create report directory: {report_dir:?}"))?;

        // Write JSON report
        let json_path = report_dir.join("comparison_report.json");
        let json_content = serde_json::to_string_pretty(report)
            .wrap_err("Failed to serialize comparison report to JSON")?;
        fs::write(&json_path, json_content)
            .wrap_err_with(|| format!("Failed to write JSON report: {json_path:?}"))?;

        // Write CSV report for per-block comparisons
        let csv_path = report_dir.join("per_block_comparison.csv");
        let mut writer = csv::Writer::from_path(&csv_path)
            .wrap_err_with(|| format!("Failed to create CSV writer: {csv_path:?}"))?;

        for comparison in &report.per_block_comparisons {
            writer.serialize(comparison).wrap_err("Failed to write comparison row to CSV")?;
        }
        writer.flush().wrap_err("Failed to flush CSV writer")?;

        info!("Comparison reports written to: {:?}", report_dir);
        Ok(())
    }

    /// Print comparison summary to console
    fn print_comparison_summary(&self, report: &ComparisonReport) {
        // Parse and format timestamp nicely
        let formatted_timestamp = if let Ok(dt) = chrono::DateTime::parse_from_str(
            &format!("{} +0000", report.timestamp.replace('_', " ")),
            "%Y%m%d %H%M%S %z",
        ) {
            dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
        } else {
            // Fallback to original if parsing fails
            report.timestamp.clone()
        };

        println!("\n=== BENCHMARK COMPARISON SUMMARY ===");
        println!("Timestamp: {formatted_timestamp}");
        println!("Baseline: {}", report.baseline.ref_name);
        println!("Feature:  {}", report.feature.ref_name);
        println!();

        let summary = &report.comparison_summary;

        println!("Performance Changes:");
        println!("  NewPayload Latency: {:+.2}%", summary.new_payload_latency_change_percent);
        println!("  FCU Latency:        {:+.2}%", summary.fcu_latency_change_percent);
        println!("  Total Latency:      {:+.2}%", summary.total_latency_change_percent);
        println!("  Gas/Second:         {:+.2}%", summary.gas_per_second_change_percent);
        println!("  Blocks/Second:      {:+.2}%", summary.blocks_per_second_change_percent);
        println!();

        println!("Baseline Summary:");
        let baseline = &report.baseline.summary;
        println!(
            "  Blocks: {}, Gas: {}, Duration: {:.2}s",
            baseline.total_blocks,
            baseline.total_gas_used,
            baseline.total_duration_ms as f64 / 1000.0
        );
        println!(
            "  Avg NewPayload: {:.2}ms, Avg FCU: {:.2}ms, Avg Total: {:.2}ms",
            baseline.avg_new_payload_latency_ms,
            baseline.avg_fcu_latency_ms,
            baseline.avg_total_latency_ms
        );
        println!();

        println!("Feature Summary:");
        let feature = &report.feature.summary;
        println!(
            "  Blocks: {}, Gas: {}, Duration: {:.2}s",
            feature.total_blocks,
            feature.total_gas_used,
            feature.total_duration_ms as f64 / 1000.0
        );
        println!(
            "  Avg NewPayload: {:.2}ms, Avg FCU: {:.2}ms, Avg Total: {:.2}ms",
            feature.avg_new_payload_latency_ms,
            feature.avg_fcu_latency_ms,
            feature.avg_total_latency_ms
        );
        println!();
    }
}
