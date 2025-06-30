//! CLI argument parsing and main command orchestration.

use clap::Parser;
use eyre::{eyre, Result};
use reth_chainspec::Chain;
use reth_cli_runner::CliContext;
use reth_node_core::args::LogArgs;
use reth_tracing::FileWorkerGuard;
use std::path::PathBuf;
use tracing::info;

use crate::{
    benchmark::BenchmarkRunner, comparison::ComparisonGenerator, compilation::CompilationManager, git::GitManager, node::NodeManager,
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

    /// Reth datadir path
    #[arg(long, value_name = "PATH")]
    pub datadir: Option<String>,

    /// Number of blocks to benchmark
    #[arg(long, value_name = "N", default_value = "100")]
    pub blocks: u64,

    /// RPC endpoint for fetching block data
    #[arg(long, value_name = "URL", default_value = "https://reth-ethereum.ithaca.xyz/rpc")]
    pub rpc_url: String,

    /// JWT secret file path
    #[arg(long, value_name = "PATH")]
    pub jwt_secret: PathBuf,

    /// Output directory for benchmark results
    #[arg(long, value_name = "PATH", default_value = "./benchmark-comparison")]
    pub output_dir: String,

    /// Skip git branch validation (useful for testing)
    #[arg(long)]
    pub skip_git_validation: bool,

    /// Skip reth compilation (use existing binaries)
    #[arg(long)]
    pub skip_compilation: bool,

    /// Compile reth-bench (by default, only reth is compiled)
    #[arg(long)]
    pub compile_reth_bench: bool,

    /// Port for reth metrics endpoint
    #[arg(long, value_name = "PORT", default_value = "5005")]
    pub metrics_port: u16,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain name or numeric chain ID.
    #[arg(
        long,
        value_name = "CHAIN",
        default_value = "mainnet",
        required = false,
    )]
    pub chain: Chain,

    /// Run reth binary with sudo (for elevated privileges)
    #[arg(long)]
    pub sudo: bool,

    /// Generate comparison charts using Python script
    #[arg(long)]
    pub draw: bool,

    #[command(flatten)]
    pub logs: LogArgs,
}

impl Args {
    /// Initializes tracing with the configured options.
    pub fn init_tracing(&self) -> Result<Option<FileWorkerGuard>> {
        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }

    /// Get the JWT secret path
    pub fn jwt_secret_path(&self) -> PathBuf {
        let jwt_secret_str = self.jwt_secret.to_string_lossy();
        let expanded = shellexpand::tilde(&jwt_secret_str);
        PathBuf::from(expanded.as_ref())
    }

    /// Get the expanded datadir path if specified
    pub fn datadir_path(&self) -> Option<PathBuf> {
        self.datadir.as_ref().map(|path| {
            let expanded = shellexpand::tilde(path);
            PathBuf::from(expanded.as_ref())
        })
    }

    /// Get the expanded output directory path
    pub fn output_dir_path(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.output_dir);
        PathBuf::from(expanded.as_ref())
    }
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
    let compilation_manager = CompilationManager::new(git_manager.repo_root().to_string());
    let node_manager = NodeManager::new(&args);
    let benchmark_runner = BenchmarkRunner::new(&args);
    let mut comparison_generator = ComparisonGenerator::new(&args);

    // Store original git state for restoration
    let original_branch = git_manager.get_current_branch()?;
    info!("Current branch: {}", original_branch);

    // Validate git state
    if !args.skip_git_validation {
        git_manager.validate_clean_state()?;
        git_manager.fetch_all()?;
        git_manager.validate_refs(&[&args.baseline_ref, &args.feature_ref])?;
    }

    // Setup signal handling for cleanup
    let git_manager_cleanup = git_manager.clone();
    let original_branch_cleanup = original_branch.clone();
    ctrlc::set_handler(move || {
        eprintln!("Received interrupt signal, cleaning up...");
        if let Err(e) = git_manager_cleanup.switch_branch(&original_branch_cleanup) {
            eprintln!("Failed to restore original branch: {}", e);
        }
        std::process::exit(1);
    })?;

    let result = run_benchmark_workflow(
        &git_manager,
        &compilation_manager,
        &node_manager,
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

    info!("Benchmark comparison completed successfully!");
    Ok(())
}

/// Execute the complete benchmark workflow for both branches
async fn run_benchmark_workflow(
    git_manager: &GitManager,
    compilation_manager: &CompilationManager,
    node_manager: &NodeManager,
    benchmark_runner: &BenchmarkRunner,
    comparison_generator: &mut ComparisonGenerator,
    args: &Args,
) -> Result<()> {
    let refs = [&args.baseline_ref, &args.feature_ref];
    let ref_types = ["baseline", "feature"];
    let mut baseline_csv_path: Option<PathBuf> = None;

    for (i, &git_ref) in refs.iter().enumerate() {
        let ref_type = ref_types[i];
        info!("=== Processing {} reference: {} ===", ref_type, git_ref);

        // Switch to target reference
        git_manager.switch_ref(git_ref)?;

        // Compile reth (always) and reth-bench (only if requested)
        if !args.skip_compilation {
            compilation_manager.compile_reth()?;
            if args.compile_reth_bench {
                compilation_manager.compile_reth_bench()?;
            }
        }

        // Start reth node
        let mut node_process = node_manager.start_node().await?;

        // Wait for node to be ready and get its current tip (wherever it is)
        let current_tip = node_manager.wait_for_node_ready_and_get_tip().await?;
        info!("Node is ready at tip: {}", current_tip);

        // Store the tip we'll unwind back to
        let original_tip = current_tip;

        // Calculate benchmark range
        let from_block = current_tip;
        let to_block = current_tip + args.blocks - 1;

        // Run benchmark
        let output_dir = comparison_generator.get_ref_output_dir(ref_type);

        if ref_type == "baseline" {
            // Run baseline benchmark without comparison
            benchmark_runner.run_benchmark(from_block, to_block, &output_dir).await?;
            baseline_csv_path = Some(output_dir.join("combined_latency.csv"));
        } else {
            // Run feature benchmark with baseline comparison
            if let Some(ref baseline_csv) = baseline_csv_path {
                benchmark_runner
                    .run_benchmark_with_baseline(from_block, to_block, &output_dir, baseline_csv)
                    .await?;
            } else {
                return Err(eyre!("Baseline CSV not available for feature reference comparison"));
            }
        }

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
    let mut cmd = tokio::process::Command::new("uv");
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
