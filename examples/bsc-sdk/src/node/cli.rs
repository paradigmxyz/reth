use crate::{
    chainspec::{parser::BscChainSpecParser, BscChainSpec},
    node::{consensus::BscConsensus, execute::BscExecutorProvider, BscNode},
};
use clap::Parser;
use reth::{
    args::LogArgs,
    builder::{NodeBuilder, WithLaunchContext},
    cli::Commands,
    prometheus_exporter::install_prometheus_recorder,
    version::{LONG_VERSION, SHORT_VERSION},
    CliRunner,
};
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::node::NoArgs;
use reth_db::DatabaseEnv;
use reth_network::EthNetworkPrimitives;
use reth_tracing::FileWorkerGuard;
use std::{
    fmt::{self},
    future::Future,
    sync::Arc,
};
use tracing::info;

/// The main reth_bsc cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli<Spec: ChainSpecParser = BscChainSpecParser, Ext: clap::Args + fmt::Debug = NoArgs> {
    /// The command to run
    #[command(subcommand)]
    pub command: Commands<Spec, Ext>,

    #[command(flatten)]
    logs: LogArgs,
}

impl<C, Ext> Cli<C, Ext>
where
    C: ChainSpecParser<ChainSpec = BscChainSpec>,
    Ext: clap::Args + fmt::Debug,
{
    /// Execute the configured cli command.
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](reth_cli_commands::node::NodeCommand).
    pub fn run<L, Fut>(self, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        self.with_runner(CliRunner::try_default_runtime()?, launcher)
    }

    /// Execute the configured cli command with the provided [`CliRunner`].
    pub fn with_runner<L, Fut>(mut self, runner: CliRunner, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        // Add network name if available to the logs dir
        if let Some(chain_spec) = self.command.chain_spec() {
            self.logs.log_file_directory =
                self.logs.log_file_directory.join(chain_spec.chain().to_string());
        }

        let _guard = self.init_tracing()?;
        info!(target: "reth::cli", "Initialized tracing, debug log directory: {}", self.logs.log_file_directory);

        // Install the prometheus recorder to be sure to record all metrics
        let _ = install_prometheus_recorder();

        let components = |spec: Arc<C::ChainSpec>| {
            (BscExecutorProvider::new(spec.clone()), BscConsensus::new(spec))
        };

        match self.command {
            Commands::Node(command) => {
                runner.run_command_until_exit(|ctx| command.execute(ctx, launcher))
            }
            Commands::Init(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<BscNode>())
            }
            Commands::InitState(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<BscNode>())
            }
            Commands::DumpGenesis(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Db(command) => runner.run_blocking_until_ctrl_c(command.execute::<BscNode>()),
            Commands::Stage(command) => runner.run_command_until_exit(|ctx| {
                command.execute::<BscNode, _, _, EthNetworkPrimitives>(ctx, components)
            }),
            Commands::P2P(command) => {
                runner.run_until_ctrl_c(command.execute::<EthNetworkPrimitives>())
            }
            Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Recover(command) => {
                runner.run_command_until_exit(|ctx| command.execute::<BscNode>(ctx))
            }
            Commands::Prune(command) => runner.run_until_ctrl_c(command.execute::<BscNode>()),
            Commands::Import(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<BscNode, _, _>(components))
            }
            Commands::Debug(_command) => todo!(),
            #[cfg(feature = "dev")]
            Commands::TestVectors(_command) => todo!(),
            Commands::ImportEra(_command) => {
                todo!()
            }
        }
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }
}
