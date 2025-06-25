//! Benchmark execution using reth-bench.

use crate::cli::Args;
use eyre::{eyre, Result, WrapErr};
use std::path::Path;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tracing::info;

/// Manages benchmark execution using reth-bench
pub struct BenchmarkRunner {
    rpc_url: String,
    jwt_secret: String,
}

impl BenchmarkRunner {
    /// Create a new BenchmarkRunner from CLI arguments
    pub fn new(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            jwt_secret: args.jwt_secret_path().to_string_lossy().to_string(),
        }
    }

    /// Run a benchmark for the specified block range
    pub async fn run_benchmark(
        &self,
        from_block: u64,
        to_block: u64,
        output_dir: &Path,
    ) -> Result<()> {
        info!(
            "Running benchmark from block {} to {} (output: {:?})",
            from_block, to_block, output_dir
        );

        // Ensure output directory exists
        std::fs::create_dir_all(output_dir)
            .wrap_err_with(|| format!("Failed to create output directory: {:?}", output_dir))?;

        // Build the reth-bench command
        let mut cmd = Command::new("reth-bench");
        cmd.args([
            "new-payload-fcu",
            "--rpc-url",
            &self.rpc_url,
            "--jwt-secret",
            &self.jwt_secret,
            "--from",
            &from_block.to_string(),
            "--to",
            &to_block.to_string(),
            "--output",
            &output_dir.to_string_lossy(),
        ]);

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        info!("Executing: {:?}", cmd);

        // Execute the benchmark
        let mut child = cmd.spawn().wrap_err("Failed to start reth-bench process")?;

        // Stream stdout with prefix
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("[RETH-BENCH] {}", line);
                }
            });
        }

        // Stream stderr with prefix
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[RETH-BENCH] {}", line);
                }
            });
        }

        let status = child.wait().await.wrap_err("Failed to wait for reth-bench")?;

        if !status.success() {
            return Err(eyre!("reth-bench failed with exit code: {:?}", status.code()));
        }

        // Verify that expected output files were created
        self.verify_benchmark_output(output_dir)?;

        info!("Benchmark completed successfully");
        Ok(())
    }

    /// Verify that the benchmark produced expected output files
    fn verify_benchmark_output(&self, output_dir: &Path) -> Result<()> {
        let expected_files = ["combined_latency.csv", "total_gas.csv"];

        for filename in &expected_files {
            let file_path = output_dir.join(filename);
            if !file_path.exists() {
                return Err(eyre!("Expected benchmark output file not found: {:?}", file_path));
            }

            // Check that the file is not empty
            let metadata = std::fs::metadata(&file_path)
                .wrap_err_with(|| format!("Failed to read metadata for {:?}", file_path))?;

            if metadata.len() == 0 {
                return Err(eyre!("Benchmark output file is empty: {:?}", file_path));
            }
        }

        info!("Verified benchmark output files");
        Ok(())
    }

    /// Run a benchmark with baseline comparison
    pub async fn run_benchmark_with_baseline(
        &self,
        from_block: u64,
        to_block: u64,
        output_dir: &Path,
        baseline_csv: &Path,
    ) -> Result<()> {
        info!(
            "Running benchmark with baseline comparison from block {} to {} (output: {:?}, baseline: {:?})",
            from_block, to_block, output_dir, baseline_csv
        );

        // Ensure output directory exists
        std::fs::create_dir_all(output_dir)
            .wrap_err_with(|| format!("Failed to create output directory: {:?}", output_dir))?;

        // Verify baseline file exists
        if !baseline_csv.exists() {
            return Err(eyre!("Baseline CSV file not found: {:?}", baseline_csv));
        }

        // Build the reth-bench command with baseline
        let mut cmd = Command::new("reth-bench");
        cmd.args([
            "new-payload-fcu",
            "--rpc-url",
            &self.rpc_url,
            "--jwt-secret",
            &self.jwt_secret,
            "--from",
            &from_block.to_string(),
            "--to",
            &to_block.to_string(),
            "--output",
            &output_dir.to_string_lossy(),
            "--baseline",
            &baseline_csv.to_string_lossy(),
        ]);

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        info!("Executing: {:?}", cmd);

        // Execute the benchmark
        let mut child = cmd.spawn().wrap_err("Failed to start reth-bench process")?;

        // Stream stdout with prefix
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("[RETH-BENCH-BASELINE] {}", line);
                }
            });
        }

        // Stream stderr with prefix
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[RETH-BENCH-BASELINE] {}", line);
                }
            });
        }

        let status = child.wait().await.wrap_err("Failed to wait for reth-bench")?;

        if !status.success() {
            return Err(eyre!("reth-bench failed with exit code: {:?}", status.code()));
        }

        // Verify that expected output files were created (including baseline comparison)
        self.verify_benchmark_output_with_baseline(output_dir)?;

        info!("Benchmark with baseline comparison completed successfully");
        Ok(())
    }

    /// Verify benchmark output including baseline comparison file
    fn verify_benchmark_output_with_baseline(&self, output_dir: &Path) -> Result<()> {
        let expected_files = ["combined_latency.csv", "total_gas.csv", "baseline_comparison.csv"];

        for filename in &expected_files {
            let file_path = output_dir.join(filename);
            if !file_path.exists() {
                return Err(eyre!("Expected benchmark output file not found: {:?}", file_path));
            }

            // Check that the file is not empty
            let metadata = std::fs::metadata(&file_path)
                .wrap_err_with(|| format!("Failed to read metadata for {:?}", file_path))?;

            if metadata.len() == 0 {
                return Err(eyre!("Benchmark output file is empty: {:?}", file_path));
            }
        }

        info!("Verified benchmark output files including baseline comparison");
        Ok(())
    }
}
