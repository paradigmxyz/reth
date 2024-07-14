//! CLI definition and entrypoint to executable

use clap::{Parser, Subcommand};
use reth::{
    args::{
        utils::{chain_help, chain_value_parser, SUPPORTED_CHAINS},
        LogArgs,
    },
    version::{LONG_VERSION, SHORT_VERSION},
};
use reth_chainspec::ChainSpec;
use reth_cli_commands::node::{self, NoArgs};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use std::{future::Future, sync::Arc};

/// Entrypoint.
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli {
    /// The command to run
    #[command(subcommand)]
    command: Commands,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = chain_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

    #[command(flatten)]
    logs: LogArgs,
}

impl Cli {
    pub fn run<L, Fut>(mut self, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>>>, NoArgs) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        // add network name to logs dir
        self.logs.log_file_directory =
            self.logs.log_file_directory.join(self.chain.chain.to_string());

        let _guard = self.logs.init_tracing()?;
        let runner = CliRunner::default();
        match self.command {
            Commands::Node(command) => {
                runner.run_command_until_exit(|ctx| command.execute(ctx, launcher))
            }
            Commands::Generator(command) => runner.run_command_until_exit(|_| command.execute()),
        }
    }
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the node
    #[command(name = "node")]
    Node(node::NodeCommand<NoArgs>),
    /// Generate stealth addresses, meta addresses and stealth address private keys.
    #[command(name = "gen")]
    Generator(Box<crate::generator::Command>),
}
