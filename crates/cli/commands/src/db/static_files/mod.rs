//! Static file related CLI commands

mod split;

pub use split::SplitCommand;

use clap::{Parser, Subcommand};
use reth_db_common::DbTool;
use reth_provider::providers::ProviderNodeTypes;

/// Static files subcommands
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Debug, Subcommand)]
enum Subcommands {
    /// Split static files into new files with different blocks-per-file setting
    Split(SplitCommand),
}

impl Command {
    /// Execute the static files command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.command {
            Subcommands::Split(cmd) => cmd.execute(tool),
        }
    }
}
