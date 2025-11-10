//! Node management for starting, stopping, and controlling reth instances.

use crate::cli::Args;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::SyncStatus;
use eyre::{eyre, OptionExt, Result, WrapErr};
#[cfg(unix)]
use nix::sys::signal::{killpg, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use reth_chainspec::Chain;
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    fs::File as AsyncFile,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader},
    process::Command,
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};

/// Manages reth node lifecycle and operations
pub(crate) struct NodeManager {
    datadir: Option<String>,
    metrics_port: u16,
    chain: Chain,
    use_sudo: bool,
    binary_path: Option<std::path::PathBuf>,
    enable_profiling: bool,
    output_dir: PathBuf,
    additional_reth_args: Vec<String>,
    comparison_dir: Option<PathBuf>,
    tracing_endpoint: Option<String>,
}

impl NodeManager {
    /// Create a new `NodeManager` with configuration from CLI args
    pub(crate) fn new(args: &Args) -> Self {
        Self {
            datadir: Some(args.datadir_path().to_string_lossy().to_string()),
            metrics_port: args.metrics_port,
            chain: args.chain,
            use_sudo: args.sudo,
            binary_path: None,
            enable_profiling: args.profile,
            output_dir: args.output_dir_path(),
            additional_reth_args: args.reth_args.clone(),
            comparison_dir: None,
            tracing_endpoint: args.traces.otlp.as_ref().map(|u| u.to_string()),
        }
    }

    /// Set the comparison directory path for logging
    pub(crate) fn set_comparison_dir(&mut self, dir: PathBuf) {
        self.comparison_dir = Some(dir);
    }

    /// Get the log file path for a given reference type
    fn get_log_file_path(&self, ref_type: &str) -> Result<PathBuf> {
        let comparison_dir = self
            .comparison_dir
            .as_ref()
            .ok_or_eyre("Comparison directory not set. Call set_comparison_dir first.")?;

        // The comparison directory already contains the full path to results/<timestamp>
        let log_dir = comparison_dir.join(ref_type);

        // Create the directory if it doesn't exist
        fs::create_dir_all(&log_dir)
            .wrap_err(format!("Failed to create log directory: {:?}", log_dir))?;

        let log_file = log_dir.join("reth_node.log");
        Ok(log_file)
    }

    /// Get the perf event max sample rate from the system, capped at 10000
    fn get_perf_sample_rate(&self) -> Option<String> {
        let perf_rate_file = "/proc/sys/kernel/perf_event_max_sample_rate";
        if let Ok(content) = fs::read_to_string(perf_rate_file) {
            let rate_str = content.trim();
            if !rate_str.is_empty() {
                if let Ok(system_rate) = rate_str.parse::<u32>() {
                    let capped_rate = std::cmp::min(system_rate, 10000);
                    info!(
                        "Detected perf_event_max_sample_rate: {}, using: {}",
                        system_rate, capped_rate
                    );
                    return Some(capped_rate.to_string());
                }
                warn!("Failed to parse perf_event_max_sample_rate: {}", rate_str);
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

        Ok(samply_path)
    }

    /// Build reth arguments as a vector of strings
    fn build_reth_args(
        &self,
        binary_path_str: &str,
        additional_args: &[String],
    ) -> (Vec<String>, String) {
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
            "--disable-discovery".to_string(),
            "--trusted-only".to_string(),
        ]);

        // Add tracing arguments if OTLP endpoint is configured
        if let Some(ref endpoint) = self.tracing_endpoint {
            info!("Enabling OTLP tracing export to: {}", endpoint);
            reth_args.push(format!("--tracing-otlp={}", endpoint));
        }

        // Add any additional arguments passed via command line (common to both baseline and
        // feature)
        reth_args.extend_from_slice(&self.additional_reth_args);

        // Add reference-specific additional arguments
        reth_args.extend_from_slice(additional_args);

        (reth_args, chain_str)
    }

    /// Create a command for profiling mode
    async fn create_profiling_command(
        &self,
        ref_type: &str,
        reth_args: &[String],
    ) -> Result<Command> {
        // Create profiles directory if it doesn't exist
        let profile_dir = self.output_dir.join("profiles");
        fs::create_dir_all(&profile_dir).wrap_err("Failed to create profiles directory")?;

        let profile_path = profile_dir.join(format!("{}.json.gz", ref_type));
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
    pub(crate) async fn start_node(
        &mut self,
        binary_path: &std::path::Path,
        _git_ref: &str,
        ref_type: &str,
        additional_args: &[String],
    ) -> Result<tokio::process::Child> {
        // Store the binary path for later use (e.g., in unwind_to_block)
        self.binary_path = Some(binary_path.to_path_buf());

        let binary_path_str = binary_path.to_string_lossy();
        let (reth_args, _) = self.build_reth_args(&binary_path_str, additional_args);

        // Log additional arguments if any
        if !self.additional_reth_args.is_empty() {
            info!("Using common additional reth arguments: {:?}", self.additional_reth_args);
        }
        if !additional_args.is_empty() {
            info!("Using reference-specific additional reth arguments: {:?}", additional_args);
        }

        let mut cmd = if self.enable_profiling {
            self.create_profiling_command(ref_type, &reth_args).await?
        } else {
            self.create_direct_command(&reth_args)
        };

        // Set process group for better signal handling
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        // Set OpenTelemetry service name to differentiate baseline vs feature runs in Jaeger
        if self.tracing_endpoint.is_some() {
            cmd.env("OTEL_SERVICE_NAME", format!("reth-{}", ref_type));
            // Set high queue size to prevent trace dropping during benchmarks for higher block
            // ranges
            cmd.env("OTEL_BLRP_MAX_QUEUE_SIZE", "10000");
            info!("OTLP tracing configured with service name: reth-{}", ref_type);
        }

        debug!("Executing reth command: {cmd:?}");

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true) // Kill on drop so that on Ctrl-C for parent process we stop all child processes
            .spawn()
            .wrap_err("Failed to start reth node")?;

        info!(
            "Reth node started with PID: {:?} (binary: {})",
            child.id().ok_or_eyre("Reth node is not running")?,
            binary_path_str
        );

        // Prepare log file path
        let log_file_path = self.get_log_file_path(ref_type)?;
        info!("Reth node logs will be saved to: {:?}", log_file_path);

        // Stream stdout and stderr with prefixes at debug level and to log file
        if let Some(stdout) = child.stdout.take() {
            let log_file = AsyncFile::create(&log_file_path)
                .await
                .wrap_err(format!("Failed to create log file: {:?}", log_file_path))?;
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stdout);
                let mut lines = reader.lines();
                let mut log_file = log_file;
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH] {}", line);
                    // Write to log file (reth already includes timestamps)
                    let log_line = format!("{}\n", line);
                    if let Err(e) = log_file.write_all(log_line.as_bytes()).await {
                        debug!("Failed to write to log file: {}", e);
                    }
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let log_file = AsyncFile::options()
                .create(true)
                .append(true)
                .open(&log_file_path)
                .await
                .wrap_err(format!("Failed to open log file for stderr: {:?}", log_file_path))?;
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stderr);
                let mut lines = reader.lines();
                let mut log_file = log_file;
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH] {}", line);
                    // Write to log file (reth already includes timestamps)
                    let log_line = format!("{}\n", line);
                    if let Err(e) = log_file.write_all(log_line.as_bytes()).await {
                        debug!("Failed to write to log file: {}", e);
                    }
                }
            });
        }

        // Give the node a moment to start up
        sleep(Duration::from_secs(5)).await;

        Ok(child)
    }

    /// Wait for the node to be ready and return its current tip
    pub(crate) async fn wait_for_node_ready_and_get_tip(&self) -> Result<u64> {
        info!("Waiting for node to be ready and synced...");

        let max_wait = Duration::from_secs(120); // 2 minutes to allow for sync
        let check_interval = Duration::from_secs(2);
        let rpc_url = "http://localhost:8545";

        // Create Alloy provider
        let url = rpc_url.parse().map_err(|e| eyre!("Invalid RPC URL '{}': {}", rpc_url, e))?;
        let provider = ProviderBuilder::new().connect_http(url);

        timeout(max_wait, async {
            loop {
                // First check if RPC is up and node is not syncing
                match provider.syncing().await {
                    Ok(sync_result) => {
                        match sync_result {
                            SyncStatus::Info(sync_info) => {
                                debug!("Node is still syncing {sync_info:?}, waiting...");
                            }
                            _ => {
                                // Node is not syncing, now get the tip
                                match provider.get_block_number().await {
                                    Ok(tip) => {
                                        info!("Node is ready and not syncing at block: {}", tip);
                                        return Ok(tip);
                                    }
                                    Err(e) => {
                                        debug!("Failed to get block number: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Node RPC not ready yet or failed to check sync status: {}", e);
                    }
                }

                sleep(check_interval).await;
            }
        })
        .await
        .wrap_err("Timed out waiting for node to be ready and synced")?
    }

    /// Stop the reth node gracefully
    pub(crate) async fn stop_node(&self, child: &mut tokio::process::Child) -> Result<()> {
        let pid = child.id().expect("Child process ID should be available");

        // Check if the process has already exited
        match child.try_wait() {
            Ok(Some(status)) => {
                info!("Reth node (PID: {}) has already exited with status: {:?}", pid, status);
                return Ok(());
            }
            Ok(None) => {
                // Process is still running, proceed to stop it
                info!("Stopping process gracefully with SIGINT (PID: {})...", pid);
            }
            Err(e) => {
                return Err(eyre!("Failed to check process status: {}", e));
            }
        }

        #[cfg(unix)]
        {
            // Send SIGINT to process group to mimic Ctrl-C behavior
            let nix_pgid = Pid::from_raw(pid as i32);

            match killpg(nix_pgid, Signal::SIGINT) {
                Ok(()) => {}
                Err(nix::errno::Errno::ESRCH) => {
                    info!("Process group {} has already exited", pid);
                }
                Err(e) => {
                    return Err(eyre!("Failed to send SIGINT to process group {}: {}", pid, e));
                }
            }
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
                // Check if the error is because the process doesn't exist
                if stderr.contains("not found") || stderr.contains("not exist") {
                    info!("Process {} has already exited", pid);
                } else {
                    return Err(eyre!("Failed to kill process {}: {}", pid, stderr));
                }
            }
        }

        // Wait for the process to exit
        match child.wait().await {
            Ok(status) => {
                info!("Reth node (PID: {}) exited with status: {:?}", pid, status);
            }
            Err(e) => {
                // If we get an error here, it might be because the process already exited
                debug!("Error waiting for process exit (may have already exited): {}", e);
            }
        }

        Ok(())
    }

    /// Unwind the node to a specific block
    pub(crate) async fn unwind_to_block(&self, block_number: u64) -> Result<()> {
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

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .wrap_err("Failed to start unwind command")?;

        // Stream stdout and stderr with prefixes in real-time
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH-UNWIND] {}", line);
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = AsyncBufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("[RETH-UNWIND] {}", line);
                }
            });
        }

        // Wait for the command to complete
        let status = child.wait().await.wrap_err("Failed to wait for unwind command")?;

        if !status.success() {
            return Err(eyre!("Unwind command failed with exit code: {:?}", status.code()));
        }

        info!("Unwound to block: {}", block_number);
        Ok(())
    }
}
