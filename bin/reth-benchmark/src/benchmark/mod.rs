//! `reth benchmark` command. Collection of various benchmarking routines.

use clap::{Parser, Subcommand};
use reth_cli_runner::CliContext;

mod local;
mod rpc;

/// `reth benchmark` command
#[derive(Debug, Parser)]
pub struct BenchmarkCommand {
    #[command(subcommand)]
    command: Subcommands,
}

/// `reth benchmark` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Generated benchmark data will come from a local database and static file folder.
    FromLocal(local::Command),
    /// Generated benchmark data will come from a remote RPC API.
    FromRpc(rpc::Command),
}

impl BenchmarkCommand {
    /// Execute `benchmark` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        match self.command {
            Subcommands::FromLocal(command) => command.execute(ctx).await,
            Subcommands::FromRpc(command) => command.execute(ctx).await,
        }
    }
}
