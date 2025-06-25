//! Node management for starting, stopping, and controlling reth instances.

use crate::cli::Args;
use eyre::{eyre, Result, WrapErr};
use reqwest::Client;
use serde_json::{json, Value};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

/// Manages reth node lifecycle and operations
pub struct NodeManager {
    datadir: String,
    jwt_secret: String,
    metrics_port: u16,
    chain: String,
    http_client: Client,
}

impl NodeManager {
    /// Create a new NodeManager with configuration from CLI args
    pub fn new(args: &Args) -> Self {
        Self {
            datadir: args.datadir_path().to_string_lossy().to_string(),
            jwt_secret: args.jwt_secret_path().to_string_lossy().to_string(),
            metrics_port: args.metrics_port,
            chain: args.chain.clone(),
            http_client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Get the current chain tip by querying an existing node or RPC endpoint
    pub async fn get_chain_tip(&self) -> Result<u64> {
        // Try to get tip from local metrics endpoint first (if node is running)
        if let Ok(tip) = self.get_tip_from_metrics().await {
            return Ok(tip);
        }

        // Fallback to querying the local RPC endpoint
        self.get_tip_from_rpc("http://localhost:8545").await
    }

    /// Start a reth node and return the process handle
    pub async fn start_node(&self) -> Result<Child> {
        info!("Starting reth node...");

        let child = Command::new("./target/profiling/reth")
            .args([
                "node",
                "--chain",
                &self.chain,
                "--engine.accept-execution-requests-hash",
                "--metrics",
                &format!("0.0.0.0:{}", self.metrics_port),
                "--http",
                "--http.api",
                "eth",
                "--datadir",
                &self.datadir,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .wrap_err("Failed to start reth node")?;

        info!("Reth node started with PID: {}", child.id());

        // Give the node a moment to start up
        sleep(Duration::from_secs(5)).await;

        Ok(child)
    }

    /// Wait for the node to sync and return the current tip
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

    /// Stop the reth node gracefully
    pub async fn stop_node(&self, child: &mut Child) -> Result<()> {
        info!("Stopping reth node...");

        // Send SIGTERM for graceful shutdown
        #[cfg(unix)]
        {
            use std::process;
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
        }

        #[cfg(windows)]
        {
            child.kill().wrap_err("Failed to kill reth process")?;
        }

        // Wait for the process to exit
        let exit_timeout = Duration::from_secs(30);
        let result = timeout(exit_timeout, async {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        info!("Reth node exited with status: {:?}", status);
                        return Ok(());
                    }
                    Ok(None) => {
                        sleep(Duration::from_millis(500)).await;
                    }
                    Err(e) => return Err(eyre!("Error waiting for process: {}", e)),
                }
            }
        })
        .await;

        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                warn!("Node didn't exit gracefully, force killing...");
                child.kill().wrap_err("Failed to force kill reth process")?;
                Ok(())
            }
        }
    }

    /// Unwind the node to a specific block
    pub async fn unwind_to_block(&self, block_number: u64) -> Result<()> {
        info!("Unwinding node to block: {}", block_number);

        let output = Command::new("./target/profiling/reth")
            .args([
                "stage",
                "unwind",
                "to-block",
                &block_number.to_string(),
                "--chain",
                &self.chain,
                "--datadir",
                &self.datadir,
            ])
            .output()
            .wrap_err("Failed to execute unwind command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Unwind command failed: {}", stderr));
        }

        info!("Successfully unwound to block: {}", block_number);
        Ok(())
    }

    /// Get chain tip from node metrics endpoint
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
