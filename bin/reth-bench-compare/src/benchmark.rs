//! Benchmark execution using reth-bench.

use crate::cli::Args;
use eyre::{eyre, Result, WrapErr};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tracing::{debug, error, info};

/// Manages benchmark execution using reth-bench
pub struct BenchmarkRunner {
    rpc_url: String,
    jwt_secret: String,
    wait_time: Option<String>,
}

impl BenchmarkRunner {
    /// Create a new BenchmarkRunner from CLI arguments
    pub fn new(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            jwt_secret: args.jwt_secret_path().to_string_lossy().to_string(),
            wait_time: args.wait_time.clone(),
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
            .wrap_err_with(|| format!("Failed to create output directory: {output_dir:?}"))?;

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

        // Add wait-time argument if provided
        if let Some(ref wait_time) = self.wait_time {
            cmd.args(["--wait-time", wait_time]);
        }

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Set process group for consistent signal handling
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        // Debug log the command
        debug!("Executing reth-bench command: {:?}", cmd);

        // Execute the benchmark
        let mut child = cmd.spawn().wrap_err("Failed to start reth-bench process")?;

        // Capture stdout and stderr for error reporting
        let stdout_lines = Arc::new(Mutex::new(Vec::new()));
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));

        // Stream stdout with prefix at debug level and capture for error reporting
        if let Some(stdout) = child.stdout.take() {
            let stdout_lines_clone = stdout_lines.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH-BENCH] {}", line);
                    if let Ok(mut captured) = stdout_lines_clone.lock() {
                        captured.push(line);
                    }
                }
            });
        }

        // Stream stderr with prefix at debug level and capture for error reporting
        if let Some(stderr) = child.stderr.take() {
            let stderr_lines_clone = stderr_lines.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH-BENCH] {}", line);
                    if let Ok(mut captured) = stderr_lines_clone.lock() {
                        captured.push(line);
                    }
                }
            });
        }

        let status = child.wait().await.wrap_err("Failed to wait for reth-bench")?;

        if !status.success() {
            // Print all captured output when command fails
            error!("reth-bench failed with exit code: {:?}", status.code());

            if let Ok(stdout) = stdout_lines.lock() {
                if !stdout.is_empty() {
                    error!("reth-bench stdout:");
                    for line in stdout.iter() {
                        error!("  {}", line);
                    }
                }
            }

            if let Ok(stderr) = stderr_lines.lock() {
                if !stderr.is_empty() {
                    error!("reth-bench stderr:");
                    for line in stderr.iter() {
                        error!("  {}", line);
                    }
                }
            }

            return Err(eyre!("reth-bench failed with exit code: {:?}", status.code()));
        }

        info!("Benchmark completed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use clap::Parser;

    #[test]
    fn test_wait_time_argument_passed() {
        // Test with wait-time provided
        let args = Args::try_parse_from([
            "reth-bench-compare",
            "--baseline-ref",
            "main",
            "--feature-ref",
            "test",
            "--wait-time",
            "200ms",
        ])
        .unwrap();

        let runner = BenchmarkRunner::new(&args);
        assert_eq!(runner.wait_time, Some("200ms".to_string()));

        // Test without wait-time
        let args = Args::try_parse_from([
            "reth-bench-compare",
            "--baseline-ref",
            "main",
            "--feature-ref",
            "test",
        ])
        .unwrap();

        let runner = BenchmarkRunner::new(&args);
        assert_eq!(runner.wait_time, None);
    }
}
