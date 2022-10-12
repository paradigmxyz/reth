use clap::{ArgAction, Parser, Subcommand};

use crate::test_eth_chain;

/// main function that parses cli and runs command
pub async fn run() -> eyre::Result<()> {
    let opt = Cli::parse();

    match opt.command {
        Commands::TestEthChain(command) => command.execute().await,
    }
}

/// Commands to be executed
#[derive(Subcommand)]
pub enum Commands {
    /// Runs Ethereum blockchain tests
    #[command(name = "test-chain")]
    TestEthChain(test_eth_chain::Command),
}

#[derive(Parser)]
#[command(author, version="0.1", about="Reth binary", long_about = None)]
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
