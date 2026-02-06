//! `reth benchmark` command. Collection of various benchmarking routines.

use clap::{Parser, Subcommand};
use reth_cli_runner::CliContext;
use reth_node_core::args::LogArgs;
use reth_tracing::FileWorkerGuard;

mod context;
mod gas_limit_ramp;
mod generate_big_block;
pub(crate) mod helpers;
pub use generate_big_block::{
    RawTransaction, RpcTransactionSource, TransactionCollector, TransactionSource,
};
mod new_payload_fcu;
mod new_payload_only;
mod output;
mod persistence_waiter;
mod replay_payloads;
mod send_invalid_payload;
mod send_payload;

/// `reth bench` command
#[derive(Debug, Parser)]
pub struct BenchmarkCommand {
    #[command(subcommand)]
    command: Subcommands,

    #[command(flatten)]
    logs: LogArgs,
}

/// `reth benchmark` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Benchmark which calls `newPayload`, then `forkchoiceUpdated`.
    NewPayloadFcu(new_payload_fcu::Command),

    /// Benchmark which builds empty blocks with a ramped gas limit.
    GasLimitRamp(gas_limit_ramp::Command),

    /// Benchmark which only calls subsequent `newPayload` calls.
    NewPayloadOnly(new_payload_only::Command),

    /// Command for generating and sending an `engine_newPayload` request constructed from an RPC
    /// block.
    ///
    /// This command takes a JSON block input (either from a file or stdin) and generates
    /// an execution payload that can be used with the `engine_newPayloadV*` API.
    ///
    /// One powerful use case is pairing this command with the `cast block` command, for example:
    ///
    /// `cast block latest --full --json | reth-bench send-payload --rpc-url localhost:5000
    /// --jwt-secret $(cat ~/.local/share/reth/mainnet/jwt.hex)`
    SendPayload(send_payload::Command),

    /// Generate a large block by packing transactions from existing blocks.
    ///
    /// This command fetches transactions from real blocks and packs them into a single
    /// block using the `testing_buildBlockV1` RPC endpoint.
    ///
    /// Example:
    ///
    /// `reth-bench generate-big-block --rpc-url http://localhost:8545 --engine-rpc-url
    /// http://localhost:8551 --jwt-secret ~/.local/share/reth/mainnet/jwt.hex --target-gas
    /// 30000000`
    GenerateBigBlock(generate_big_block::Command),

    /// Replay pre-generated payloads from a directory.
    ///
    /// This command reads payload files from a previous `generate-big-block` run and replays
    /// them in sequence using `newPayload` followed by `forkchoiceUpdated`.
    ///
    /// Example:
    ///
    /// `reth-bench replay-payloads --payload-dir ./payloads --engine-rpc-url
    /// http://localhost:8551 --jwt-secret ~/.local/share/reth/mainnet/jwt.hex`
    ReplayPayloads(replay_payloads::Command),

    /// Generate and send an invalid `engine_newPayload` request for testing.
    ///
    /// Takes a valid block and modifies fields to make it invalid, allowing you to test
    /// Engine API rejection behavior. Block hash is recalculated after modifications
    /// unless `--invalid-block-hash` or `--skip-hash-recalc` is used.
    ///
    /// Example:
    ///
    /// `cast block latest --full --json | reth-bench send-invalid-payload --rpc-url localhost:5000
    /// --jwt-secret $(cat ~/.local/share/reth/mainnet/jwt.hex) --invalid-state-root`
    SendInvalidPayload(Box<send_invalid_payload::Command>),
}

impl BenchmarkCommand {
    /// Execute `benchmark` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        // Initialize tracing
        let _guard = self.init_tracing()?;

        match self.command {
            Subcommands::NewPayloadFcu(command) => command.execute(ctx).await,
            Subcommands::GasLimitRamp(command) => command.execute(ctx).await,
            Subcommands::NewPayloadOnly(command) => command.execute(ctx).await,
            Subcommands::SendPayload(command) => command.execute(ctx).await,
            Subcommands::GenerateBigBlock(command) => command.execute(ctx).await,
            Subcommands::ReplayPayloads(command) => command.execute(ctx).await,
            Subcommands::SendInvalidPayload(command) => (*command).execute(ctx).await,
        }
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    ///
    /// Always enables log target display (`RUST_LOG_TARGET=1`) so that the `reth-bench` target
    /// is visible in output, making it easy to distinguish reth-bench logs from reth logs when
    /// both are streamed to the same console or file.
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
        // Always show the log target so "reth-bench" is visible in the output.
        if std::env::var_os("RUST_LOG_TARGET").is_none() {
            // SAFETY: This is called early during single-threaded initialization, before any
            // threads are spawned and before the tracing subscriber is set up.
            unsafe { std::env::set_var("RUST_LOG_TARGET", "1") };
        }

        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }
}
