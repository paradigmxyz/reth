//! `reth benchmark` command. Collection of various benchmarking routines.

use clap::{Parser, Subcommand};
use reth_cli_runner::CliContext;
use reth_node_core::args::LogArgs;
use reth_tracing::FileWorkerGuard;

mod context;
mod new_payload_fcu;
mod new_payload_only;
mod output;

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

    /// Benchmark which only calls subsequent `newPayload` calls.
    NewPayloadOnly(new_payload_only::Command),
}

impl BenchmarkCommand {
    /// Execute `benchmark` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        // Initialize tracing
        let _guard = self.init_tracing()?;

        match self.command {
            Subcommands::NewPayloadFcu(command) => command.execute(ctx).await,
            Subcommands::NewPayloadOnly(command) => command.execute(ctx).await,
        }
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }
}
