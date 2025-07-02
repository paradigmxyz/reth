//! Node management for starting, stopping, and controlling reth instances.

use crate::{cli::Args, git::sanitize_git_ref};
use eyre::{eyre, Result, WrapErr};
#[cfg(unix)]
use nix::sys::signal::{kill, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use reqwest::Client;
use reth_chainspec::Chain;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader as AsyncBufReader},
    process::Command,
    time::{sleep, timeout},
};
use tracing::{debug, error, info};

/// Manages reth node lifecycle and operations
pub struct NodeManager {
    datadir: Option<String>,
    metrics_port: u16,
    chain: Chain,
    use_sudo: bool,
    http_client: Client,
    binary_path: Option<std::path::PathBuf>,
    enable_profiling: bool,
    output_dir: PathBuf,
}

impl NodeManager {
    /// Create a new NodeManager with configuration from CLI args
    pub fn new(args: &Args) -> Self {
        Self {
            datadir: Some(args.datadir_path().to_string_lossy().to_string()),
            metrics_port: args.metrics_port,
            chain: args.chain,
            use_sudo: args.sudo,
            http_client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            binary_path: None,
            enable_profiling: args.profile,
            output_dir: args.output_dir_path(),
        }
    }

    /// Get the perf event max sample rate from the system
    fn get_perf_sample_rate(&self) -> Option<String> {
        let perf_rate_file = "/proc/sys/kernel/perf_event_max_sample_rate";
        if let Ok(content) = fs::read_to_string(perf_rate_file) {
            let rate = content.trim();
            if !rate.is_empty() {
                info!("Detected perf_event_max_sample_rate: {}", rate);
                return Some(rate.to_string());
            }
        }
        None
    }

    /// Get the absolute path to samply using 'which' command
    async fn get_samply_path(&self) -> Result<String> {
        let output = Command::new("which")
            .arg("samply")
            .output()
            .await
            .wrap_err("Failed to execute 'which samply' command")?;

        if !output.status.success() {
            return Err(eyre!("samply not found in PATH"));
        }

        let samply_path = String::from_utf8(output.stdout)
            .wrap_err("samply path is not valid UTF-8")?
            .trim()
            .to_string();

        if samply_path.is_empty() {
            return Err(eyre!("which samply returned empty path"));
        }

        info!("Found samply at: {}", samply_path);
        Ok(samply_path)
    }

    /// Build reth arguments as a vector of strings
    fn build_reth_args(&self, binary_path_str: &str) -> (Vec<String>, String) {
        let mut reth_args = vec![binary_path_str.to_string(), "node".to_string()];

        // Add chain argument (skip for mainnet as it's the default)
        let chain_str = self.chain.to_string();
        if chain_str != "mainnet" {
            reth_args.extend_from_slice(&["--chain".to_string(), chain_str.clone()]);
        }

        // Add datadir if specified
        if let Some(ref datadir) = self.datadir {
            reth_args.extend_from_slice(&["--datadir".to_string(), datadir.clone()]);
        }

        // Add reth-specific arguments
        let metrics_arg = format!("0.0.0.0:{}", self.metrics_port);
        reth_args.extend_from_slice(&[
            "--engine.accept-execution-requests-hash".to_string(),
            "--metrics".to_string(),
            metrics_arg,
            "--http".to_string(),
            "--http.api".to_string(),
            "eth".to_string(),
        ]);

        (reth_args, chain_str)
    }

    /// Create a command for profiling mode
    async fn create_profiling_command(
        &self,
        git_ref: &str,
        reth_args: &[String],
    ) -> Result<Command> {
        // Create profiles directory if it doesn't exist
        let profile_dir = self.output_dir.join("profiles");
        fs::create_dir_all(&profile_dir).wrap_err("Failed to create profiles directory")?;

        let profile_path = profile_dir.join(format!("{}.json.gz", sanitize_git_ref(git_ref)));
        info!("Starting reth node with samply profiling...");
        info!("Profile output: {:?}", profile_path);

        // Get absolute path to samply
        let samply_path = self.get_samply_path().await?;

        let mut cmd = if self.use_sudo {
            let mut sudo_cmd = Command::new("sudo");
            sudo_cmd.arg(&samply_path);
            sudo_cmd
        } else {
            Command::new(&samply_path)
        };

        // Add samply arguments
        cmd.args(["record", "--save-only", "-o", &profile_path.to_string_lossy()]);

        // Add rate argument if available
        if let Some(rate) = self.get_perf_sample_rate() {
            cmd.args(["--rate", &rate]);
        }

        // Add separator and complete reth command
        cmd.arg("--");
        cmd.args(reth_args);

        // Set process group for better signal handling when profiling
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        Ok(cmd)
    }

    /// Create a command for direct reth execution
    fn create_direct_command(&self, reth_args: &[String]) -> Command {
        let binary_path = &reth_args[0];

        if self.use_sudo {
            info!("Starting reth node with sudo...");
            let mut cmd = Command::new("sudo");
            cmd.args(reth_args);

            cmd
        } else {
            info!("Starting reth node...");
            let mut cmd = Command::new(binary_path);
            cmd.args(&reth_args[1..]); // Skip the binary path since it's the command

            cmd
        }
    }

    /// Start a reth node using the specified binary path and return the process handle
    pub async fn start_node(
        &mut self,
        binary_path: &std::path::Path,
        git_ref: &str,
    ) -> Result<tokio::process::Child> {
        // Store the binary path for later use (e.g., in unwind_to_block)
        self.binary_path = Some(binary_path.to_path_buf());

        let binary_path_str = binary_path.to_string_lossy();
        let (reth_args, _) = self.build_reth_args(&binary_path_str);

        let mut cmd = if self.enable_profiling {
            self.create_profiling_command(git_ref, &reth_args).await?
        } else {
            self.create_direct_command(&reth_args)
        };
        debug!("Executing reth command: {cmd:?}");

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true) // Kill on drop so that on Ctrl-C for parent process we stop all child processes
            .spawn()
            .wrap_err("Failed to start reth node")?;

        info!("Reth node started with PID: {:?} (binary: {})", child.id(), binary_path_str);

        // Stream stdout and stderr with prefixes at debug level
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH] {}", line);
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH] {}", line);
                }
            });
        }

        // Give the node a moment to start up
        sleep(Duration::from_secs(5)).await;

        Ok(child)
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
        
        #[cfg(unix)]
        {
            // Use nix crate to send SIGINT to the process group on Unix systems
            // Mimic Ctrl-C: negative value == process group id
            let pgid = -(pid as i32);
            let nix_pgid = Pid::from_raw(pgid);
            kill(nix_pgid, Signal::SIGINT)
                .wrap_err_with(|| format!("Failed to send SIGINT to process group {}", pid))?;
        }
        
        #[cfg(not(unix))]
        {
            // On non-Unix systems, fall back to using external kill command
            let output = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output()
                .await
                .wrap_err("Failed to execute taskkill command")?;
                
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(eyre!("Failed to kill process {}: {}", pid, stderr));
            }
        }

        // Wait for the process to exit
        let status = child.wait().await.wrap_err("Failed to wait for reth process to exit")?;
        info!("Reth node (PID: {}) exited with status: {:?}", pid, status);

        Ok(())
    }

    /// Unwind the node to a specific block
    pub async fn unwind_to_block(&self, block_number: u64) -> Result<()> {
        if self.use_sudo {
            info!("Unwinding node to block: {} (with sudo)", block_number);
        } else {
            info!("Unwinding node to block: {}", block_number);
        }

        // Use the binary path from the last start_node call, or fallback to default
        let binary_path = self
            .binary_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "./target/profiling/reth".to_string());

        let mut cmd = if self.use_sudo {
            let mut sudo_cmd = Command::new("sudo");
            sudo_cmd.args([&binary_path, "stage", "unwind"]);
            sudo_cmd
        } else {
            let mut reth_cmd = Command::new(&binary_path);
            reth_cmd.args(["stage", "unwind"]);
            reth_cmd
        };

        // Add chain argument (skip for mainnet as it's the default)
        let chain_str = self.chain.to_string();
        if chain_str != "mainnet" {
            cmd.args(["--chain", &chain_str]);
        }

        // Add datadir if specified
        if let Some(ref datadir) = self.datadir {
            cmd.args(["--datadir", datadir]);
        }

        cmd.args(["to-block", &block_number.to_string()]);

        // Debug log the command
        debug!("Executing reth unwind command: {:?}", cmd);

        let output = cmd.output().await.wrap_err("Failed to execute unwind command")?;

        // Print stdout and stderr with prefixes at debug level
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        for line in stdout.lines() {
            if !line.trim().is_empty() {
                debug!("[RETH-UNWIND] {}", line);
            }
        }

        for line in stderr.lines() {
            if !line.trim().is_empty() {
                debug!("[RETH-UNWIND] {}", line);
            }
        }

        if !output.status.success() {
            // Print all output when unwind fails
            error!("Reth unwind failed with exit code: {:?}", output.status.code());

            if !stdout.trim().is_empty() {
                error!("Reth unwind stdout:");
                for line in stdout.lines() {
                    error!("  {}", line);
                }
            }

            if !stderr.trim().is_empty() {
                error!("Reth unwind stderr:");
                for line in stderr.lines() {
                    error!("  {}", line);
                }
            }

            return Err(eyre!("Unwind command failed with exit code: {:?}", output.status.code()));
        }

        info!("Successfully unwound to block: {}", block_number);
        Ok(())
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
