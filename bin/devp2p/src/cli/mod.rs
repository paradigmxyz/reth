use crate::commands::key;

use clap::{Parser, Subcommand};
use eyre::Ok;

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

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
    #[command(name = "key")]
    Key(key::KeyCommand),
}
