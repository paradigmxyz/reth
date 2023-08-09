//! `reth recover` command.
use clap::{Parser, Subcommand};

use crate::runner::CliContext;

mod storage_tries;

/// `reth recover` command
#[derive(Debug, Parser)]
pub struct Command {
    #[clap(subcommand)]
    command: Subcommands,
}

/// `reth recover` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Recover the node by deleting dangling storage tries.
    StorageTries(storage_tries::Command),
}

impl Command {
    /// Execute `recover` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        match self.command {
            Subcommands::StorageTries(command) => command.execute(ctx).await,
        }
    }
}
