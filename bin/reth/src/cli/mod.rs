//! CLI definition and entrypoint to executable

use crate::{
    args::LogArgs,
    commands::debug_cmd,
    version::{LONG_VERSION, SHORT_VERSION},
};
use clap::{value_parser, Parser, Subcommand};
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::{
    config_cmd, db, dump_genesis, import, init_cmd, init_state,
    node::{self, NoArgs},
    p2p, prune, recover, stage,
};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_node_ethereum::{EthExecutorProvider, EthereumNode};
use reth_tracing::FileWorkerGuard;
use std::{ffi::OsString, fmt, future::Future, sync::Arc};
use tracing::info;

/// Re-export of the `reth_node_core` types specifically in the `cli` module.
///
/// This is re-exported because the types in `reth_node_core::cli` originally existed in
/// `reth::cli` but were moved to the `reth_node_core` crate. This re-export avoids a breaking
/// change.
pub use crate::core::cli::*;

/// The main reth cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli<C: ChainSpecParser = EthereumChainSpecParser, Ext: clap::Args + fmt::Debug = NoArgs>
{
    /// The command to run
    #[command(subcommand)]
    command: Commands<C, Ext>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = C::help_message(),
        default_value = C::SUPPORTED_CHAINS[0],
        value_parser = C::parser(),
        global = true,
    )]
    chain: Arc<C::ChainSpec>,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - `DISCOVERY_PORT`: default + `instance` - 1
    /// - `AUTH_PORT`: default + `instance` * 100 - 100
    /// - `HTTP_RPC_PORT`: default - `instance` + 1
    /// - `WS_RPC_PORT`: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    instance: u16,

    #[command(flatten)]
    logs: LogArgs,
}

impl Cli {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default CLI arguments from the given iterator
    pub fn try_parse_args_from<I, T>(itr: I) -> Result<Self, clap::error::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Self::try_parse_from(itr)
    }
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>, Ext: clap::Args + fmt::Debug> Cli<C, Ext> {
    /// Execute the configured cli command.
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](node::NodeCommand).
    ///
    ///
    /// # Example
    ///
    /// ```no_run
    /// use reth::cli::Cli;
    /// use reth_node_ethereum::EthereumNode;
    ///
    /// Cli::parse_args()
    ///     .run(|builder, _| async move {
    ///         let handle = builder.launch_node(EthereumNode::default()).await?;
    ///
    ///         handle.wait_for_node_exit().await
    ///     })
    ///     .unwrap();
    /// ```
    ///
    /// # Example
    ///
    /// Parse additional CLI arguments for the node command and use it to configure the node.
    ///
    /// ```no_run
    /// use clap::Parser;
    /// use reth::cli::Cli;
    /// use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
    ///
    /// #[derive(Debug, Parser)]
    /// pub struct MyArgs {
    ///     pub enable: bool,
    /// }
    ///
    /// Cli::<EthereumChainSpecParser, MyArgs>::parse()
    ///     .run(|builder, my_args: MyArgs| async move {
    ///         // launch the node
    ///
    ///         Ok(())
    ///     })
    ///     .unwrap();
    /// ````
    pub fn run<L, Fut>(mut self, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        // add network name to logs dir
        self.logs.log_file_directory =
            self.logs.log_file_directory.join(self.chain.chain.to_string());

        let _guard = self.init_tracing()?;
        info!(target: "reth::cli", "Initialized tracing, debug log directory: {}", self.logs.log_file_directory);

        let runner = CliRunner::default();
        match self.command {
            Commands::Node(command) => {
                runner.run_command_until_exit(|ctx| command.execute(ctx, launcher))
            }
            Commands::Init(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<EthereumNode>())
            }
            Commands::InitState(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<EthereumNode>())
            }
            Commands::Import(command) => runner.run_blocking_until_ctrl_c(
                command.execute::<EthereumNode, _, _>(EthExecutorProvider::ethereum),
            ),
            Commands::DumpGenesis(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Db(command) => {
                runner.run_blocking_until_ctrl_c(command.execute::<EthereumNode>())
            }
            Commands::Stage(command) => runner.run_command_until_exit(|ctx| {
                command.execute::<EthereumNode, _, _>(ctx, EthExecutorProvider::ethereum)
            }),
            Commands::P2P(command) => runner.run_until_ctrl_c(command.execute()),
            #[cfg(feature = "dev")]
            Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Debug(command) => {
                runner.run_command_until_exit(|ctx| command.execute::<EthereumNode>(ctx))
            }
            Commands::Recover(command) => {
                runner.run_command_until_exit(|ctx| command.execute::<EthereumNode>(ctx))
            }
            Commands::Prune(command) => runner.run_until_ctrl_c(command.execute::<EthereumNode>()),
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

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands<C: ChainSpecParser, Ext: clap::Args + fmt::Debug> {
    /// Start the node
    #[command(name = "node")]
    Node(Box<node::NodeCommand<C, Ext>>),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(init_cmd::InitCommand<C>),
    /// Initialize the database from a state dump file.
    #[command(name = "init-state")]
    InitState(init_state::InitStateCommand<C>),
    /// This syncs RLP encoded blocks from a file.
    #[command(name = "import")]
    Import(import::ImportCommand<C>),
    /// Dumps genesis block JSON configuration to stdout.
    DumpGenesis(dump_genesis::DumpGenesisCommand<C>),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command<C>),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(stage::Command<C>),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command<C>),
    /// Generate Test Vectors
    #[cfg(feature = "dev")]
    #[command(name = "test-vectors")]
    TestVectors(reth_cli_commands::test_vectors::Command),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config_cmd::Command),
    /// Various debug routines
    #[command(name = "debug")]
    Debug(debug_cmd::Command<C>),
    /// Scripts for node recovery
    #[command(name = "recover")]
    Recover(recover::Command<C>),
    /// Prune according to the configuration without any limits
    #[command(name = "prune")]
    Prune(prune::PruneCommand<C>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ColorMode;
    use clap::CommandFactory;
    use reth_ethereum_cli::chainspec::SUPPORTED_CHAINS;

    #[test]
    fn parse_color_mode() {
        let reth = Cli::try_parse_args_from(["reth", "node", "--color", "always"]).unwrap();
        assert_eq!(reth.logs.color, ColorMode::Always);
    }

    /// Tests that the help message is parsed correctly. This ensures that clap args are configured
    /// correctly and no conflicts are introduced via attributes that would result in a panic at
    /// runtime
    #[test]
    fn test_parse_help_all_subcommands() {
        let reth = Cli::<EthereumChainSpecParser, NoArgs>::command();
        for sub_command in reth.get_subcommands() {
            let err = Cli::try_parse_args_from(["reth", sub_command.get_name(), "--help"])
                .err()
                .unwrap_or_else(|| {
                    panic!("Failed to parse help message {}", sub_command.get_name())
                });

            // --help is treated as error, but
            // > Not a true "error" as it means --help or similar was used. The help message will be sent to stdout.
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        }
    }

    /// Tests that the log directory is parsed correctly. It's always tied to the specific chain's
    /// name
    #[test]
    fn parse_logs_path() {
        let mut reth = Cli::try_parse_args_from(["reth", "node"]).unwrap();
        reth.logs.log_file_directory =
            reth.logs.log_file_directory.join(reth.chain.chain.to_string());
        let log_dir = reth.logs.log_file_directory;
        let end = format!("reth/logs/{}", SUPPORTED_CHAINS[0]);
        assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");

        let mut iter = SUPPORTED_CHAINS.iter();
        iter.next();
        for chain in iter {
            let mut reth = Cli::try_parse_args_from(["reth", "node", "--chain", chain]).unwrap();
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(reth.chain.chain.to_string());
            let log_dir = reth.logs.log_file_directory;
            let end = format!("reth/logs/{chain}");
            assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");
        }
    }

    #[test]
    fn parse_env_filter_directives() {
        let temp_dir = tempfile::tempdir().unwrap();

        std::env::set_var("RUST_LOG", "info,evm=debug");
        let reth = Cli::try_parse_args_from([
            "reth",
            "init",
            "--datadir",
            temp_dir.path().to_str().unwrap(),
            "--log.file.filter",
            "debug,net=trace",
        ])
        .unwrap();
        assert!(reth.run(|_, _| async move { Ok(()) }).is_ok());
    }
}
