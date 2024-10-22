//! Command for generating test vectors.

use clap::{Parser, Subcommand};

mod compact;
mod tables;

/// Generate test-vectors for different data types.
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug)]
/// `reth test-vectors` subcommands
pub enum Subcommands {
    /// Generates test vectors for specified tables. If no table is specified, generate for all.
    Tables {
        /// List of table names. Case-sensitive.
        names: Vec<String>,
    },
    /// Generates test vectors for `Compact` types with `--write`. Reads and checks generated
    /// vectors with `--read`.
    #[group(multiple = false, required = true)]
    Compact {
        /// Write test vectors to a file.
        #[arg(long)]
        write: bool,

        /// Read test vectors from a file.
        #[arg(long)]
        read: bool,
    },
}

impl Command {
    /// Execute the command
    pub async fn execute(self) -> eyre::Result<()> {
        match self.command {
            Subcommands::Tables { names } => {
                tables::generate_vectors(names)?;
            }
            Subcommands::Compact { write, .. } => {
                if write {
                    compact::generate_vectors()?;
                } else {
                    compact::read_vectors()?;
                }
            }
        }
        Ok(())
    }
}
