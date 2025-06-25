//! CLI argument parsing and main command orchestration.

use clap::Parser;
use eyre::{eyre, Result};
use reth_cli_runner::CliContext;
use reth_node_core::args::LogArgs;
use reth_tracing::FileWorkerGuard;
use std::path::PathBuf;
use tracing::info;

use crate::{
    benchmark::BenchmarkRunner, comparison::ComparisonGenerator, git::GitManager, node::NodeManager,
};

/// Automated reth benchmark comparison between git branches
#[derive(Debug, Parser)]
#[command(
    name = "reth-bench-compare",
    about = "Compare reth performance between two git branches",
    version
)]
pub struct Args {
    /// Git branch to use as baseline for comparison
    #[arg(long, value_name = "BRANCH")]
    pub baseline_branch: String,

    /// Git branch to compare against the baseline
    #[arg(long, value_name = "BRANCH")]
    pub feature_branch: String,

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

    /// Port for reth metrics endpoint
    #[arg(long, value_name = "PORT", default_value = "5005")]
    pub metrics_port: u16,

    /// Chain to use for reth operations (mainnet, sepolia, holesky, etc.)
    #[arg(long, value_name = "CHAIN", default_value = "mainnet")]
    pub chain: String,

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
        args.baseline_branch, args.feature_branch
    );

    // Initialize managers
    let git_manager = GitManager::new()?;
    let node_manager = NodeManager::new(&args);
    let benchmark_runner = BenchmarkRunner::new(&args);
    let mut comparison_generator = ComparisonGenerator::new(&args);

    // Store original git state for restoration
    let original_branch = git_manager.get_current_branch()?;
    info!("Current branch: {}", original_branch);

    // Validate git state
    if !args.skip_git_validation {
        git_manager.validate_clean_state()?;
        git_manager.validate_branches(&[&args.baseline_branch, &args.feature_branch])?;
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
    node_manager: &NodeManager,
    benchmark_runner: &BenchmarkRunner,
    comparison_generator: &mut ComparisonGenerator,
    args: &Args,
) -> Result<()> {
    let branches = [&args.baseline_branch, &args.feature_branch];
    let branch_names = ["baseline", "feature"];
    let mut baseline_csv_path: Option<PathBuf> = None;

    for (i, &branch) in branches.iter().enumerate() {
        let branch_type = branch_names[i];
        info!("=== Processing {} branch: {} ===", branch_type, branch);

        // Switch to target branch
        git_manager.switch_branch(branch)?;

        // Compile reth and reth-bench
        if !args.skip_compilation {
            git_manager.compile_reth()?;
            git_manager.compile_reth_bench()?;
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
        let output_dir = comparison_generator.get_branch_output_dir(branch_type);

        if branch_type == "baseline" {
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
                return Err(eyre!("Baseline CSV not available for feature branch comparison"));
            }
        }

        // Stop node
        node_manager.stop_node(&mut node_process).await?;

        // Unwind back to original tip
        node_manager.unwind_to_block(original_tip).await?;

        // Store results for comparison
        comparison_generator.add_branch_results(branch_type, &output_dir)?;

        info!("Completed {} branch benchmark", branch_type);
    }

    // Generate comparison report
    comparison_generator.generate_comparison_report().await?;

    Ok(())
}
