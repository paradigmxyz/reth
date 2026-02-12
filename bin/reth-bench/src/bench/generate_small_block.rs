//! Command for generating small blocks (under 20M gas) for benchmarking.
//!
//! This is a thin wrapper around `generate-big-block` with a lower default gas target
//! and no gas limit ramp phase required. Useful for benchmarking small block performance.

use crate::bench::{generate_big_block, helpers::parse_gas_limit};
use clap::Parser;
use reth_cli_runner::CliContext;

/// `reth bench generate-small-block` command
///
/// Generates a small block by fetching transactions from existing blocks and packing them
/// into a block with low gas usage (default 15M) using the `testing_buildBlockV1` RPC endpoint.
///
/// Unlike `generate-big-block`, this does not require a prior gas limit ramp phase since
/// the target gas is below the standard 30M block gas limit.
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC URL to use for fetching blocks (can be an external archive node).
    #[arg(long, value_name = "RPC_URL")]
    rpc_url: String,

    /// The engine RPC URL (with JWT authentication).
    #[arg(long, value_name = "ENGINE_RPC_URL", default_value = "http://localhost:8551")]
    engine_rpc_url: String,

    /// The RPC URL for `testing_buildBlockV1` calls (same node as engine, regular RPC port).
    #[arg(long, value_name = "TESTING_RPC_URL", default_value = "http://localhost:8545")]
    testing_rpc_url: String,

    /// Path to the JWT secret file for engine API authentication.
    #[arg(long, value_name = "JWT_SECRET")]
    jwt_secret: std::path::PathBuf,

    /// Target gas to pack into the block (default 15M for small blocks).
    /// Accepts short notation: K for thousand, M for million, G for billion.
    #[arg(long, value_name = "TARGET_GAS", default_value = "15000000", value_parser = parse_gas_limit)]
    target_gas: u64,

    /// Block number to start fetching transactions from (required).
    ///
    /// Since small blocks don't require gas limit ramping, this can simply be a recent
    /// block number with real transaction activity.
    #[arg(long, value_name = "FROM_BLOCK")]
    from_block: u64,

    /// Execute the payload (call newPayload + forkchoiceUpdated).
    /// If false, only builds the payload and prints it.
    #[arg(long, default_value = "false")]
    execute: bool,

    /// Number of payloads to generate. Each payload uses the previous as parent.
    /// When count == 1, the payload is only generated and saved, not executed.
    /// When count > 1, each payload is executed before building the next.
    #[arg(long, default_value = "1")]
    count: u64,

    /// Number of transaction batches to prefetch in background when count > 1.
    /// Higher values reduce latency but use more memory.
    #[arg(long, default_value = "4")]
    prefetch_buffer: usize,

    /// Output directory for generated payloads. Each payload is saved as `payload_block_N.json`.
    #[arg(long, value_name = "OUTPUT_DIR")]
    output_dir: std::path::PathBuf,
}

impl Command {
    /// Execute the `generate-small-block` command by delegating to `generate-big-block`.
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let big_block_cmd = generate_big_block::Command {
            rpc_url: self.rpc_url,
            engine_rpc_url: self.engine_rpc_url,
            testing_rpc_url: self.testing_rpc_url,
            jwt_secret: self.jwt_secret,
            target_gas: self.target_gas,
            from_block: self.from_block,
            execute: self.execute,
            count: self.count,
            prefetch_buffer: self.prefetch_buffer,
            output_dir: self.output_dir,
        };
        big_block_cmd.execute(ctx).await
    }
}
