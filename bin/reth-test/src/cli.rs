use clap::{ArgAction, Parser, Subcommand};

use crate::ethereum_blockchain;

/// main function that parses cli and runs command
pub async fn run() -> anyhow::Result<()> {
    let opt = Cli::parse();

    match opt.command {
        Commands::EthereumBlockchain(command) => command.execute().await,
    }
}


/// Commands to be executed
#[derive(Subcommand)]
pub enum Commands {
    /// Runs ethereum blockchain tests
    EthereumBlockchain(ethereum_blockchain::Command),
}

#[derive(Parser)]
#[command(author, version="0.1", about="Reth test utility", long_about = None)]
struct Cli {
    /// The command to run
    #[clap(subcommand)]
    command: Commands,

    /// Use verbose output
    #[clap(short, long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Silence all output
    #[clap(short, long, global = true)]
    silent: bool,
}
