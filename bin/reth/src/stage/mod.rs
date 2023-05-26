//! `reth stage` command
use clap::{Parser, Subcommand};

pub mod drop;
pub mod dump;
pub mod run;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    #[clap(subcommand)]
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
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(self) -> eyre::Result<()> {
        match self.command {
            Subcommands::Run(command) => command.execute().await,
            Subcommands::Drop(command) => command.execute().await,
            Subcommands::Dump(command) => command.execute().await,
        }
    }
}
