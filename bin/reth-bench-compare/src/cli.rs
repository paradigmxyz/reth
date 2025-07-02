//! CLI argument parsing and main command orchestration.

use alloy_provider::{Provider, ProviderBuilder};
use clap::Parser;
use eyre::{eyre, Result, WrapErr};
use reth_chainspec::Chain;
use reth_cli_runner::CliContext;
use reth_node_core::args::{DatadirArgs, LogArgs};
use reth_tracing::FileWorkerGuard;
use std::{net::TcpListener, path::PathBuf};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::{
    benchmark::BenchmarkRunner, comparison::ComparisonGenerator, compilation::CompilationManager,
    git::GitManager, node::NodeManager,
};

/// Automated reth benchmark comparison between git references
#[derive(Debug, Parser)]
#[command(
    name = "reth-bench-compare",
    about = "Compare reth performance between two git references (branches or tags)",
    version
)]
pub struct Args {
    /// Git reference (branch or tag) to use as baseline for comparison
    #[arg(long, value_name = "REF")]
    pub baseline_ref: String,

    /// Git reference (branch or tag) to compare against the baseline
    #[arg(long, value_name = "REF")]
    pub feature_ref: String,

    #[command(flatten)]
    pub datadir: DatadirArgs,

    /// Number of blocks to benchmark
    #[arg(long, value_name = "N", default_value = "100")]
    pub blocks: u64,

    /// RPC endpoint for fetching block data
    #[arg(long, value_name = "URL", default_value = "https://reth-ethereum.ithaca.xyz/rpc")]
    pub rpc_url: String,

    /// JWT secret file path
    ///
    /// If not provided, defaults to `<datadir>/<chain>/jwt.hex`.
    /// If the file doesn't exist, it will be created automatically.
    #[arg(long, value_name = "PATH")]
    pub jwt_secret: Option<PathBuf>,

    /// Output directory for benchmark results
    #[arg(long, value_name = "PATH", default_value = "./reth-bench-compare")]
    pub output_dir: String,

    /// Skip git branch validation (useful for testing)
    #[arg(long)]
    pub skip_git_validation: bool,

    /// Port for reth metrics endpoint
    #[arg(long, value_name = "PORT", default_value = "5005")]
    pub metrics_port: u16,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain name or numeric chain ID.
    #[arg(long, value_name = "CHAIN", default_value = "mainnet", required = false)]
    pub chain: Chain,

    /// Run reth binary with sudo (for elevated privileges)
    #[arg(long)]
    pub sudo: bool,

    /// Generate comparison charts using Python script
    #[arg(long)]
    pub draw: bool,

    /// Enable CPU profiling with samply during benchmark runs
    #[arg(long)]
    pub profile: bool,

    /// Wait time between engine API calls (passed to reth-bench)
    #[arg(long, value_name = "DURATION")]
    pub wait_time: Option<String>,

    #[command(flatten)]
    pub logs: LogArgs,
}

impl Args {
    /// Initializes tracing with the configured options.
    pub fn init_tracing(&self) -> Result<Option<FileWorkerGuard>> {
        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }

    /// Get the JWT secret path - either provided or derived from datadir
    pub fn jwt_secret_path(&self) -> PathBuf {
        match &self.jwt_secret {
            Some(path) => {
                let jwt_secret_str = path.to_string_lossy();
                let expanded = shellexpand::tilde(&jwt_secret_str);
                PathBuf::from(expanded.as_ref())
            }
            None => {
                // Use the same logic as reth: <datadir>/<chain>/jwt.hex
                let chain_path = self.datadir.clone().resolve_datadir(self.chain);
                chain_path.jwt()
            }
        }
    }

    /// Get the resolved datadir path using the chain
    pub fn datadir_path(&self) -> PathBuf {
        let chain_path = self.datadir.clone().resolve_datadir(self.chain);
        chain_path.data_dir().to_path_buf()
    }

    /// Get the expanded output directory path
    pub fn output_dir_path(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.output_dir);
        PathBuf::from(expanded.as_ref())
    }
}

/// Validate that the RPC endpoint chain ID matches the specified chain
async fn validate_rpc_chain_id(rpc_url: &str, expected_chain: &Chain) -> Result<()> {
    // Create Alloy provider
    let url = rpc_url.parse().map_err(|e| eyre!("Invalid RPC URL '{}': {}", rpc_url, e))?;
    let provider = ProviderBuilder::new().connect_http(url);

    // Query chain ID using Alloy
    let rpc_chain_id = provider
        .get_chain_id()
        .await
        .map_err(|e| eyre!("Failed to get chain ID from RPC endpoint {}: {:?}", rpc_url, e))?;

    let expected_chain_id = expected_chain.id();

    if rpc_chain_id != expected_chain_id {
        return Err(eyre!(
            "RPC endpoint chain ID mismatch!\n\
            Expected: {} (chain: {})\n\
            Found: {} at RPC endpoint: {}\n\n\
            Please use an RPC endpoint for the correct network or change the --chain argument.",
            expected_chain_id,
            expected_chain,
            rpc_chain_id,
            rpc_url
        ));
    }

    info!("Validated RPC endpoint chain ID");
    Ok(())
}

/// Main comparison workflow execution
pub async fn run_comparison(args: Args, _ctx: CliContext) -> Result<()> {
    info!(
        "Starting benchmark comparison between '{}' and '{}'",
        args.baseline_ref, args.feature_ref
    );

    if args.sudo {
        info!("Running in sudo mode - reth commands will use elevated privileges");
    }

    // Initialize managers
    let git_manager = GitManager::new()?;
    let output_dir = args.output_dir_path();
    let compilation_manager = CompilationManager::new(
        git_manager.repo_root().to_string(),
        output_dir.clone(),
        git_manager.clone(),
    )?;
    let mut node_manager = NodeManager::new(&args);
    let benchmark_runner = BenchmarkRunner::new(&args);
    let mut comparison_generator = ComparisonGenerator::new(&args);

    // Store original git state for restoration
    let original_branch = git_manager.get_current_branch()?;
    info!("Current branch: {}", original_branch);

    // Fetch all branches, tags, and commits
    git_manager.fetch_all()?;

    // Validate git state
    if !args.skip_git_validation {
        git_manager.validate_clean_state()?;
        git_manager.validate_refs(&[&args.baseline_ref, &args.feature_ref])?;
    }

    // Validate RPC endpoint chain ID matches the specified chain
    validate_rpc_chain_id(&args.rpc_url, &args.chain).await?;

    // Setup signal handling for cleanup
    let git_manager_cleanup = git_manager.clone();
    let original_branch_cleanup = original_branch.clone();
    ctrlc::set_handler(move || {
        eprintln!("Received interrupt signal, cleaning up...");

        // Give a moment for any ongoing git operations to complete
        std::thread::sleep(std::time::Duration::from_millis(200));

        if let Err(e) = git_manager_cleanup.switch_branch(&original_branch_cleanup) {
            eprintln!("Failed to restore original branch: {e}");
            eprintln!("You may need to manually run: git checkout {}", original_branch_cleanup);
        }
        std::process::exit(1);
    })?;

    let result = run_benchmark_workflow(
        &git_manager,
        &compilation_manager,
        &mut node_manager,
        &benchmark_runner,
        &mut comparison_generator,
        &args,
    )
    .await;

    // Always restore original branch
    info!("Restoring original branch: {}", original_branch);
    git_manager.switch_branch(&original_branch)?;

    // Handle any errors from the workflow
    result?;

    Ok(())
}

/// Execute the complete benchmark workflow for both branches
async fn run_benchmark_workflow(
    git_manager: &GitManager,
    compilation_manager: &CompilationManager,
    node_manager: &mut NodeManager,
    benchmark_runner: &BenchmarkRunner,
    comparison_generator: &mut ComparisonGenerator,
    args: &Args,
) -> Result<()> {
    let refs = [&args.baseline_ref, &args.feature_ref];
    let ref_types = ["baseline", "feature"];

    for (i, &git_ref) in refs.iter().enumerate() {
        let ref_type = ref_types[i];
        info!("=== Processing {} reference: {} ===", ref_type, git_ref);

        // Switch to target reference
        git_manager.switch_ref(git_ref)?;

        // Compile reth (with caching) and ensure reth-bench is available
        compilation_manager.compile_reth(git_ref)?;

        // Always ensure reth-bench is available (compile if not found)
        compilation_manager.ensure_reth_bench_available()?;

        // Ensure samply is available if profiling is enabled
        if args.profile {
            compilation_manager.ensure_samply_available()?;
        }

        // Get the binary path for this git reference
        let binary_path = compilation_manager.get_cached_binary_path(git_ref);

        // Start reth node
        let mut node_process = node_manager.start_node(&binary_path, git_ref).await?;

        // Wait for node to be ready and get its current tip (wherever it is)
        let current_tip = node_manager.wait_for_node_ready_and_get_tip().await?;
        info!("Node is ready at tip: {}", current_tip);

        // Store the tip we'll unwind back to
        let original_tip = current_tip;

        // Calculate benchmark range
        // Note: reth-bench has an off-by-one error where it consumes the first block
        // of the range, so we add 1 to compensate and get exactly args.blocks blocks
        let from_block = current_tip;
        let to_block = current_tip + args.blocks;

        // Run benchmark
        let output_dir = comparison_generator.get_ref_output_dir(ref_type);

        // Run benchmark (comparison logic is handled separately by ComparisonGenerator)
        benchmark_runner.run_benchmark(from_block, to_block, &output_dir).await?;

        // Stop node
        node_manager.stop_node(&mut node_process).await?;

        // Unwind back to original tip
        node_manager.unwind_to_block(original_tip).await?;

        // Store results for comparison
        comparison_generator.add_ref_results(ref_type, &output_dir)?;

        info!("Completed {} reference benchmark", ref_type);
    }

    // Generate comparison report
    comparison_generator.generate_comparison_report().await?;

    // Generate charts if requested
    if args.draw {
        generate_comparison_charts(comparison_generator, args).await?;
    }

    // Start samply servers if profiling was enabled
    if args.profile {
        start_samply_servers(args).await?;
    }

    Ok(())
}

/// Generate comparison charts using the Python script
async fn generate_comparison_charts(
    comparison_generator: &ComparisonGenerator,
    _args: &Args,
) -> Result<()> {
    info!("Generating comparison charts with Python script...");

    let baseline_output_dir = comparison_generator.get_ref_output_dir("baseline");
    let feature_output_dir = comparison_generator.get_ref_output_dir("feature");

    let baseline_csv = baseline_output_dir.join("combined_latency.csv");
    let feature_csv = feature_output_dir.join("combined_latency.csv");

    // Check if CSV files exist
    if !baseline_csv.exists() {
        return Err(eyre!("Baseline CSV not found: {:?}", baseline_csv));
    }
    if !feature_csv.exists() {
        return Err(eyre!("Feature CSV not found: {:?}", feature_csv));
    }

    let output_dir = comparison_generator.get_output_dir();
    let chart_output = output_dir.join("latency_comparison.png");

    let script_path = "bin/reth-bench/scripts/compare_newpayload_latency.py";

    info!("Running Python comparison script with uv...");
    let mut cmd = Command::new("uv");
    cmd.args([
        "run",
        script_path,
        &baseline_csv.to_string_lossy(),
        &feature_csv.to_string_lossy(),
        "-o",
        &chart_output.to_string_lossy(),
    ]);

    let output = cmd.output().await.map_err(|e| {
        eyre!("Failed to execute Python script with uv: {}. Make sure uv is installed.", e)
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(eyre!(
            "Python script failed with exit code {:?}:\nstdout: {}\nstderr: {}",
            output.status.code(),
            stdout,
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        info!("Python script output:\n{}", stdout);
    }

    info!("Comparison chart generated: {:?}", chart_output);
    Ok(())
}

/// Start samply servers for viewing profiles
async fn start_samply_servers(args: &Args) -> Result<()> {
    use crate::git::sanitize_git_ref;

    info!("Starting samply servers for profile viewing...");

    let output_dir = args.output_dir_path();
    let profiles_dir = output_dir.join("profiles");

    // Build profile paths
    let baseline_profile =
        profiles_dir.join(format!("{}.json.gz", sanitize_git_ref(&args.baseline_ref)));
    let feature_profile =
        profiles_dir.join(format!("{}.json.gz", sanitize_git_ref(&args.feature_ref)));

    // Check if profiles exist
    if !baseline_profile.exists() {
        warn!("Baseline profile not found: {:?}", baseline_profile);
        return Ok(());
    }
    if !feature_profile.exists() {
        warn!("Feature profile not found: {:?}", feature_profile);
        return Ok(());
    }

    // Find two consecutive available ports starting from 3000
    let (baseline_port, feature_port) = find_consecutive_ports(3000)?;
    info!("Found available ports: {} and {}", baseline_port, feature_port);

    // Get samply path
    let samply_path = get_samply_path().await?;

    // Start baseline server
    info!("Starting samply server for baseline '{}' on port {}", args.baseline_ref, baseline_port);
    let mut baseline_cmd = Command::new(&samply_path);
    baseline_cmd
        .args(["load", "--port", &baseline_port.to_string(), &baseline_profile.to_string_lossy()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    // Debug log the command
    debug!("Executing samply load command: {:?}", baseline_cmd);

    let mut baseline_child =
        baseline_cmd.spawn().wrap_err("Failed to start samply server for baseline")?;

    // Start feature server
    info!("Starting samply server for feature '{}' on port {}", args.feature_ref, feature_port);
    let mut feature_cmd = Command::new(&samply_path);
    feature_cmd
        .args(["load", "--port", &feature_port.to_string(), &feature_profile.to_string_lossy()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    // Debug log the command
    debug!("Executing samply load command: {:?}", feature_cmd);

    let mut feature_child =
        feature_cmd.spawn().wrap_err("Failed to start samply server for feature")?;

    // Give servers time to start
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Print access information
    println!("\n=== SAMPLY PROFILE SERVERS STARTED ===");
    println!("Baseline '{}': http://127.0.0.1:{}", args.baseline_ref, baseline_port);
    println!("Feature  '{}': http://127.0.0.1:{}", args.feature_ref, feature_port);
    println!("\nOpen the URLs in your browser to view the profiles.");
    println!("Press Ctrl+C to stop the servers and exit.");
    println!("=========================================\n");

    // Wait for Ctrl+C or process termination
    let ctrl_c = tokio::signal::ctrl_c();
    let baseline_wait = baseline_child.wait();
    let feature_wait = feature_child.wait();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, shutting down samply servers...");
        }
        result = baseline_wait => {
            match result {
                Ok(status) => info!("Baseline samply server exited with status: {}", status),
                Err(e) => warn!("Baseline samply server error: {}", e),
            }
        }
        result = feature_wait => {
            match result {
                Ok(status) => info!("Feature samply server exited with status: {}", status),
                Err(e) => warn!("Feature samply server error: {}", e),
            }
        }
    }

    // Ensure both processes are terminated
    let _ = baseline_child.kill().await;
    let _ = feature_child.kill().await;

    info!("Samply servers stopped.");
    Ok(())
}

/// Find two consecutive available ports starting from the given port
fn find_consecutive_ports(start_port: u16) -> Result<(u16, u16)> {
    for port in start_port..=65533 {
        // Check if both port and port+1 are available
        if is_port_available(port) && is_port_available(port + 1) {
            return Ok((port, port + 1));
        }
    }
    Err(eyre!("Could not find two consecutive available ports starting from {}", start_port))
}

/// Check if a port is available by attempting to bind to it
fn is_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Get the absolute path to samply using 'which' command
async fn get_samply_path() -> Result<String> {
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
