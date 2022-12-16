//! CLI definition and entrypoint to executable

use clap::{ArgAction, Parser, Subcommand};
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::layers::{PrefixLayer, Stack};
use tracing_subscriber::util::SubscriberInitExt;

use crate::{
    db, node, test_eth_chain,
    util::reth_tracing::{self, TracingMode},
};

/// main function that parses cli and runs command
pub async fn run() -> eyre::Result<()> {
    let opt = Cli::parse();

    let tracing = if opt.silent { TracingMode::Silent } else { TracingMode::All };

    reth_tracing::build_subscriber(tracing).init();

    if opt.metrics {
        let (recorder, exporter) =
            PrometheusBuilder::new().build().expect("couldn't build Prometheus");
        tokio::task::spawn(exporter);
        Stack::new(recorder)
            .push(PrefixLayer::new("reth"))
            .install()
            .expect("couldn't install metrics recorder");
    }

    match opt.command {
        Commands::Node(command) => command.execute().await,
        Commands::TestEthChain(command) => command.execute().await,
        Commands::Db(command) => command.execute().await,
    }
}

/// Commands to be executed
#[derive(Subcommand)]
pub enum Commands {
    /// Main node command
    #[command(name = "node")]
    Node(node::Command),
    /// Runs Ethereum blockchain tests
    #[command(name = "test-chain")]
    TestEthChain(test_eth_chain::Command),
    /// DB Debugging utilities
    #[command(name = "db")]
    Db(db::Command),
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

    #[clap(long, global = true)]
    metrics: bool,
}
