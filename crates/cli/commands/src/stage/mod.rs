//! `reth stage` command

use std::sync::Arc;

use clap::{Parser, Subcommand};
use reth_chainspec::ChainSpec;
use reth_cli_runner::CliContext;
use reth_evm::execute::BlockExecutorProvider;

pub mod drop;
pub mod dump;
pub mod run;
pub mod unwind;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

/// `reth stage` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Run a single stage.
    ///
    /// Note that this won't use the Pipeline and as a result runs stages
    /// assuming that all the data can be held in memory. It is not recommended
    /// to run a stage for really large block ranges if your computer does not have
    /// a lot of memory to store all the data.
    Run(run::Command),
    /// Drop a stage's tables from the database.
    Drop(drop::Command),
    /// Dumps a stage from a range into a new database.
    Dump(dump::Command),
    /// Unwinds a certain block range, deleting it from the database.
    Unwind(unwind::Command),
}

impl Command {
    /// Execute `stage` command
    pub async fn execute<E, F>(self, ctx: CliContext, executor: F) -> eyre::Result<()>
    where
        E: BlockExecutorProvider,
        F: FnOnce(Arc<ChainSpec>) -> E,
    {
        match self.command {
            Subcommands::Run(command) => command.execute(ctx, executor).await,
            Subcommands::Drop(command) => command.execute().await,
            Subcommands::Dump(command) => command.execute(executor).await,
            Subcommands::Unwind(command) => command.execute().await,
        }
    }
}
