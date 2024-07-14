//! Command that initializes the node from a genesis file.
use clap::{command, Parser, Subcommand};

mod addr;
mod key;
mod meta;

/// `stealthy gen` command
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Generator,
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Generator {
    /// Generate a stealth address from a stealth meta address alongside an optional encrypted
    /// note.
    #[command(name = "addr")]
    Address(addr::Command),
    /// Generate a stealth meta address from view and spend private keys.
    #[command(name = "meta")]
    Meta(meta::Command),
    /// Generate the stealth address private key from the ephemeral public key and view & spend
    /// private keys.
    #[command(name = "key")]
    Key(key::Command),
}

impl Command {
    pub async fn execute(self) -> eyre::Result<()> {
        match self.command {
            Generator::Address(command) => command.execute().await,
            Generator::Meta(command) => command.execute().await,
            Generator::Key(command) => command.execute().await,
        }
    }
}
