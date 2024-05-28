//! CLI definition and entrypoint to executable

use crate::commands::key;

use clap::{Parser, Subcommand};
use eyre::Ok;

/// The main devp2p cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
pub struct Cli {
    /// The command to run
    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Execute the configured cli command.
    pub fn run(self) -> eyre::Result<()> {
        match self.command {
            Commands::Key(command) => {
                let _ = command.execute();
            }
        }

        Ok(())
    }
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Operations on node key.
    #[command(name = "key")]
    Key(key::KeyCommand),
}
