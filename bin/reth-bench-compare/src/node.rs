//! Node management for starting, stopping, and controlling reth instances.

use crate::cli::Args;
use eyre::{eyre, Result, WrapErr};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader as AsyncBufReader},
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

/// Manages reth node lifecycle and operations
pub struct NodeManager {
    datadir: Option<String>,
    #[allow(dead_code)]
    jwt_secret: String,
    metrics_port: u16,
    chain: Option<String>,
    use_sudo: bool,
    quiet: bool,
    http_client: Client,
}

impl NodeManager {
    /// Create a new NodeManager with configuration from CLI args
    pub fn new(args: &Args) -> Self {
        Self {
            datadir: args.datadir_path().map(|p| p.to_string_lossy().to_string()),
            jwt_secret: args.jwt_secret_path().to_string_lossy().to_string(),
            metrics_port: args.metrics_port,
            chain: if args.chain == "mainnet" { None } else { Some(args.chain.clone()) },
            use_sudo: args.sudo,
            quiet: args.quiet,
            http_client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Get the current chain tip by querying an existing node or RPC endpoint
    #[allow(dead_code)]
    pub async fn get_chain_tip(&self) -> Result<u64> {
        // Try to get tip from local metrics endpoint first (if node is running)
        if let Ok(tip) = self.get_tip_from_metrics().await {
            return Ok(tip);
        }

        // Fallback to querying the local RPC endpoint
        self.get_tip_from_rpc("http://localhost:8545").await
    }

    /// Get the current chain tip from an external RPC endpoint
    #[allow(dead_code)]
    pub async fn get_tip_from_external_rpc(&self, rpc_url: &str) -> Result<u64> {
        info!("Getting chain tip from external RPC: {}", rpc_url);
        self.get_tip_from_rpc(rpc_url).await
    }

    /// Start a reth node and return the process handle
    pub async fn start_node(&self) -> Result<tokio::process::Child> {
        if self.use_sudo {
            info!("Starting reth node with sudo...");
        } else {
            info!("Starting reth node...");
        }

        let mut cmd = if self.use_sudo {
            let mut sudo_cmd = tokio::process::Command::new("sudo");
            sudo_cmd.args(["./target/profiling/reth", "node"]);
            sudo_cmd
        } else {
            let mut reth_cmd = tokio::process::Command::new("./target/profiling/reth");
            reth_cmd.arg("node");
            reth_cmd
        };

        // Add chain if specified
        if let Some(ref chain) = self.chain {
            cmd.args(["--chain", chain]);
        }

        // Add datadir if specified
        if let Some(ref datadir) = self.datadir {
            cmd.args(["--datadir", datadir]);
        }

        cmd.args([
            "--engine.accept-execution-requests-hash",
            "--metrics",
            &format!("0.0.0.0:{}", self.metrics_port),
            "--http",
            "--http.api",
            "eth",
        ]);

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .wrap_err("Failed to start reth node")?;

        info!("Reth node started with PID: {:?}", child.id());

        // Stream stdout and stderr with prefixes (unless quiet)
        if !self.quiet {
            if let Some(stdout) = child.stdout.take() {
                tokio::spawn(async move {
                    let reader = AsyncBufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        println!("[RETH] {}", line);
                    }
                });
            }

            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(async move {
                    let reader = AsyncBufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        eprintln!("[RETH] {}", line);
                    }
                });
            }
        }

        // Give the node a moment to start up
        sleep(Duration::from_secs(5)).await;

        Ok(child)
    }

    /// Wait for the node to sync and return the current tip
    #[allow(dead_code)]
    pub async fn wait_for_sync_and_get_tip(&self) -> Result<u64> {
        info!("Waiting for node to sync...");

        let max_wait = Duration::from_secs(300); // 5 minutes
        let check_interval = Duration::from_secs(10);

        let result = timeout(max_wait, async {
            loop {
                if let Ok(tip) = self.get_tip_from_metrics().await {
                    info!("Node synced to block: {}", tip);
                    return Ok(tip);
                }

                debug!("Node not ready yet, waiting...");
                sleep(check_interval).await;
            }
        })
        .await
        .wrap_err("Timed out waiting for node to sync")?;

        result
    }

    /// Wait for the node to be ready and return its current tip (doesn't wait for sync)
    pub async fn wait_for_node_ready_and_get_tip(&self) -> Result<u64> {
        info!("Waiting for node to be ready...");

        let max_wait = Duration::from_secs(60); // 1 minute should be enough for RPC to come up
        let check_interval = Duration::from_secs(2);

        let result = timeout(max_wait, async {
            loop {
                // Try to get tip from local RPC first
                if let Ok(tip) = self.get_tip_from_rpc("http://localhost:8545").await {
                    info!("Node RPC is ready at block: {}", tip);
                    return Ok(tip);
                }

                debug!("Node RPC not ready yet, waiting...");
                sleep(check_interval).await;
            }
        })
        .await
        .wrap_err("Timed out waiting for node RPC to be ready")?;

        result
    }

    /// Stop the reth node gracefully
    pub async fn stop_node(&self, child: &mut tokio::process::Child) -> Result<()> {
        let pid = child.id().expect("Child process ID should be available");
        info!("Stopping reth node gracefully with SIGINT (PID: {})...", pid);

        // Use tokio's built-in kill for graceful shutdown
        // TODO: Should use SIGINT instead of SIGKILL for graceful shutdown, but
        // programmatic SIGINT doesn't work for some reason (manual `kill -2 PID` works fine)
        child.kill().await.wrap_err("Failed to kill reth process")?;

        // Wait for the process to exit
        let status = child.wait().await.wrap_err("Failed to wait for reth process to exit")?;
        info!("Reth node (PID: {}) exited with status: {:?}", pid, status);

        // Manually delete the storage lock files to ensure clean shutdown
        if let Some(ref datadir) = self.datadir {
            let lock_files = vec![
                std::path::Path::new(datadir).join("db").join("lock"),
                std::path::Path::new(datadir).join("static_files").join("lock"),
            ];
            
            for lock_file in lock_files {
                if lock_file.exists() {
                    if self.use_sudo {
                        // Use sudo to remove the lock file
                        match tokio::process::Command::new("sudo")
                            .args(["rm", "-f", &lock_file.to_string_lossy()])
                            .spawn()
                        {
                            Ok(mut child) => {
                                match child.wait().await {
                                    Ok(status) if status.success() => {
                                        info!("Removed storage lock file with sudo: {:?}", lock_file);
                                    }
                                    Ok(status) => {
                                        warn!("Failed to remove storage lock file with sudo {:?}: exit status {:?}", lock_file, status);
                                    }
                                    Err(e) => {
                                        warn!("Failed to wait for sudo rm process {:?}: {}", lock_file, e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to spawn sudo rm for storage lock file {:?}: {}", lock_file, e);
                            }
                        }
                    } else {
                        // Remove without sudo
                        match std::fs::remove_file(&lock_file) {
                            Ok(()) => {
                                info!("Removed storage lock file: {:?}", lock_file);
                            }
                            Err(e) => {
                                warn!("Failed to remove storage lock file {:?}: {}", lock_file, e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Unwind the node to a specific block
    pub async fn unwind_to_block(&self, block_number: u64) -> Result<()> {
        if self.use_sudo {
            info!("Unwinding node to block: {} (with sudo)", block_number);
        } else {
            info!("Unwinding node to block: {}", block_number);
        }

        let mut cmd = if self.use_sudo {
            let mut sudo_cmd = std::process::Command::new("sudo");
            sudo_cmd.args(["./target/profiling/reth", "stage", "unwind"]);
            sudo_cmd
        } else {
            let mut reth_cmd = std::process::Command::new("./target/profiling/reth");
            reth_cmd.args(["stage", "unwind"]);
            reth_cmd
        };

        // Add chain if specified
        if let Some(ref chain) = self.chain {
            cmd.args(["--chain", chain]);
        }

        // Add datadir if specified
        if let Some(ref datadir) = self.datadir {
            cmd.args(["--datadir", datadir]);
        }

        cmd.args(["to-block", &block_number.to_string()]);

        let output = cmd.output().wrap_err("Failed to execute unwind command")?;

        // Print stdout and stderr with prefixes (unless quiet)
        if !self.quiet {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            for line in stdout.lines() {
                if !line.trim().is_empty() {
                    println!("[RETH-UNWIND] {}", line);
                }
            }

            for line in stderr.lines() {
                if !line.trim().is_empty() {
                    eprintln!("[RETH-UNWIND] {}", line);
                }
            }
        }

        if !output.status.success() {
            return Err(eyre!("Unwind command failed with exit code: {:?}", output.status.code()));
        }

        info!("Successfully unwound to block: {}", block_number);
        Ok(())
    }

    /// Get chain tip from node metrics endpoint
    #[allow(dead_code)]
    async fn get_tip_from_metrics(&self) -> Result<u64> {
        let url = format!("http://localhost:{}/metrics", self.metrics_port);
        let response = self.http_client.get(&url).send().await.wrap_err("Failed to get metrics")?;

        if !response.status().is_success() {
            return Err(eyre!("Metrics endpoint returned error: {}", response.status()));
        }

        let body = response.text().await.wrap_err("Failed to read metrics response")?;

        // Parse prometheus metrics to find the chain tip
        // Look for a metric like: reth_blockchain_tip_number
        for line in body.lines() {
            if line.starts_with("reth_blockchain_tip_number") && !line.starts_with('#') {
                if let Some(value_str) = line.split_whitespace().last() {
                    if let Ok(value) = value_str.parse::<f64>() {
                        return Ok(value as u64);
                    }
                }
            }
        }

        Err(eyre!("Could not find chain tip in metrics"))
    }

    /// Get chain tip from RPC endpoint
    async fn get_tip_from_rpc(&self, rpc_url: &str) -> Result<u64> {
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });

        let response = self
            .http_client
            .post(rpc_url)
            .json(&request_body)
            .send()
            .await
            .wrap_err("Failed to send RPC request")?;

        if !response.status().is_success() {
            return Err(eyre!("RPC request failed with status: {}", response.status()));
        }

        let json: Value = response.json().await.wrap_err("Failed to parse RPC response")?;

        let result = json.get("result").ok_or_else(|| eyre!("No result field in RPC response"))?;

        let hex_str = result.as_str().ok_or_else(|| eyre!("Result is not a string"))?;

        // Remove "0x" prefix and parse as hex
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let block_number =
            u64::from_str_radix(hex_str, 16).wrap_err("Failed to parse block number from hex")?;

        Ok(block_number)
    }
}
