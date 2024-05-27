use clap::{Parser, Subcommand};

mod generate;

/// `devp2p key` command
#[derive(Debug, Parser)]
pub struct KeyCommand {
    #[command(subcommand)]
    pub command: Subcommands,
}

/// `devp2p key` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    // Generates node key files
    Generate(generate::Command),
}

impl KeyCommand {
    pub fn execute(&self) -> eyre::Result<()> {
        match &self.command {
            Subcommands::Generate(command) => {
                let _ = command.execute();
            }
        }
        Ok(())
    }
}
