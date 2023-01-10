//! CLI definition and entrypoint to executable

use crate::{db, node, p2p, stage, test_eth_chain};

use clap::{ArgAction, Parser, Subcommand};
use reth_cli_utils::reth_tracing::{self, TracingMode};
use tracing_subscriber::util::SubscriberInitExt;

/// main function that parses cli and runs command
pub async fn run() -> eyre::Result<()> {
    let opt = Cli::parse();
    reth_tracing::build_subscriber(if opt.silent {
        TracingMode::Silent
    } else {
        TracingMode::from(opt.verbose)
    })
    .init();

    match opt.command {
        Commands::Node(command) => command.execute().await,
        Commands::TestEthChain(command) => command.execute().await,
        Commands::Db(command) => command.execute().await,
        Commands::Stage(command) => command.execute().await,
        Commands::P2P(command) => command.execute().await,
    }
}

/// Commands to be executed
#[derive(Subcommand)]
pub enum Commands {
    /// Start the node
    #[command(name = "node")]
    Node(node::Command),
    /// Run Ethereum blockchain tests
    #[command(name = "test-chain")]
    TestEthChain(test_eth_chain::Command),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command),
    /// Run a single stage.
    ///
    /// Note that this won't use the Pipeline and as a result runs stages
    /// assuming that all the data can be held in memory. It is not recommended
    /// to run a stage for really large block ranges if your computer does not have
    /// a lot of memory to store all the data.
    #[command(name = "stage")]
    Stage(stage::Command),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command),
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
    #[clap(long, global = true)]
    silent: bool,
}
