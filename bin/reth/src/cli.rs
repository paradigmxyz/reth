//! CLI definition and entrypoint to executable
use crate::{
    db, node, p2p, stage, test_eth_chain,
    utils::reth_tracing::{self},
};
use clap::{ArgAction, Args, Parser, Subcommand};
use tracing::{metadata::LevelFilter, Level};
use tracing_subscriber::{filter::Directive, util::SubscriberInitExt};

/// main function that parses cli and runs command
pub async fn run() -> eyre::Result<()> {
    let opt = Cli::parse();

    reth_tracing::build_subscriber(opt.verbosity.directive()).init();

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
#[command(author, version = "0.1", about = "Reth", long_about = None)]
struct Cli {
    /// The command to run
    #[clap(subcommand)]
    command: Commands,

    #[clap(flatten)]
    verbosity: Verbosity,
}

#[derive(Args)]
#[command(next_help_heading = "Display")]
struct Verbosity {
    /// Set the minimum log level for stdout.
    ///
    /// -v      Errors
    /// -vv     Warnings
    /// -vvv    Info
    /// -vvvv   Debug
    /// -vvvvv  Traces (warning: very verbose!)
    #[clap(short, long, action = ArgAction::Count, global = true, default_value_t = 3, verbatim_doc_comment, help_heading = "Display")]
    verbosity: u8,

    /// Silence all log output.
    #[clap(long, alias = "silent", short = 'q', global = true, help_heading = "Display")]
    quiet: bool,
}

impl Verbosity {
    /// Get the corresponding [Directive] for the given verbosity, or none if the verbosity
    /// corresponds to silent.
    fn directive(&self) -> Directive {
        if self.quiet {
            LevelFilter::OFF.into()
        } else {
            let level = match self.verbosity - 1 {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                _ => Level::TRACE,
            };

            format!("reth={}", level).parse().unwrap()
        }
    }
}
