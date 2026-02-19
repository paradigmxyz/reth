//! CLI definition and entrypoint to executable

use crate::{
    app::{run_commands_with, CliApp},
    chainspec::EthereumChainSpecParser,
};
use clap::{Parser, Subcommand};
use reth_chainspec::{ChainSpec, EthChainSpec, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::{
    common::{CliComponentsBuilder, CliNodeTypes, HeaderMut},
    config_cmd, db, download, dump_genesis, export_era, import, import_era, init_cmd, init_state,
    launcher::FnLauncher,
    node::{self, NoArgs},
    p2p, prune, re_execute, stage,
};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_node_api::NodePrimitives;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_node_core::{
    args::{LogArgs, OtlpInitStatus, OtlpLogsStatus, TraceArgs},
    version::version_metadata,
};
use reth_node_metrics::recorder::install_prometheus_recorder;
use reth_rpc_server_types::{DefaultRpcModuleValidator, RpcModuleValidator};
use reth_tracing::{FileWorkerGuard, Layers};
use std::{ffi::OsString, fmt, future::Future, marker::PhantomData, sync::Arc};
use tracing::{info, warn};

/// The main reth cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
#[command(author, name = version_metadata().name_client.as_ref(), version = version_metadata().short_version.as_ref(), long_version = version_metadata().long_version.as_ref(), about = "Reth", long_about = None)]
pub struct Cli<
    C: ChainSpecParser = EthereumChainSpecParser,
    Ext: clap::Args + fmt::Debug = NoArgs,
    Rpc: RpcModuleValidator = DefaultRpcModuleValidator,
    SubCmd: Subcommand + fmt::Debug = NoSubCmd,
> {
    /// The command to run
    #[command(subcommand)]
    pub command: Commands<C, Ext, SubCmd>,

    /// The logging configuration for the CLI.
    #[command(flatten)]
    pub logs: LogArgs,

    /// The tracing configuration for the CLI.
    #[command(flatten)]
    pub traces: TraceArgs,

    /// Type marker for the RPC module validator
    #[arg(skip)]
    pub _phantom: PhantomData<Rpc>,
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

impl<
        C: ChainSpecParser,
        Ext: clap::Args + fmt::Debug,
        Rpc: RpcModuleValidator,
        SubCmd: crate::app::ExtendedCommand + Subcommand + fmt::Debug,
    > Cli<C, Ext, Rpc, SubCmd>
{
    /// Configures the CLI and returns a [`CliApp`] instance.
    ///
    /// This method is used to prepare the CLI for execution by wrapping it in a
    /// [`CliApp`] that can be further configured before running.
    pub fn configure(self) -> CliApp<C, Ext, Rpc, SubCmd> {
        CliApp::new(self)
    }

    /// Execute the configured cli command.
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](node::NodeCommand).
    ///
    /// This command will be run on the default tokio runtime.
    ///
    ///
    /// # Example
    ///
    /// ```no_run
    /// use reth_ethereum_cli::interface::Cli;
    /// use reth_node_ethereum::EthereumNode;
    ///
    /// Cli::parse_args()
    ///     .run(async move |builder, _| {
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
    /// use reth_ethereum_cli::{chainspec::EthereumChainSpecParser, interface::Cli};
    ///
    /// #[derive(Debug, Parser)]
    /// pub struct MyArgs {
    ///     pub enable: bool,
    /// }
    ///
    /// Cli::<EthereumChainSpecParser, MyArgs>::parse()
    ///     .run(async move |builder, my_args: MyArgs|
    ///         // launch the node
    ///         Ok(()))
    ///     .unwrap();
    /// ````
    pub fn run<L, Fut>(self, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
        C: ChainSpecParser<ChainSpec = ChainSpec>,
    {
        self.with_runner(CliRunner::try_default_runtime()?, launcher)
    }

    /// Execute the configured cli command with the provided [`CliComponentsBuilder`].
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](node::NodeCommand).
    ///
    /// This command will be run on the default tokio runtime.
    pub fn run_with_components<N>(
        self,
        components: impl CliComponentsBuilder<N>,
        launcher: impl AsyncFnOnce(
            WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>,
            Ext,
        ) -> eyre::Result<()>,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<Primitives: NodePrimitives<BlockHeader: HeaderMut>, ChainSpec: Hardforks>,
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        self.with_runner_and_components(CliRunner::try_default_runtime()?, components, launcher)
    }

    /// Execute the configured cli command with the provided [`CliRunner`].
    ///
    ///
    /// # Example
    ///
    /// ```no_run
    /// use reth_cli_runner::CliRunner;
    /// use reth_ethereum_cli::interface::Cli;
    /// use reth_node_ethereum::EthereumNode;
    ///
    /// let runner = CliRunner::try_default_runtime().unwrap();
    ///
    /// Cli::parse_args()
    ///     .with_runner(runner, |builder, _| async move {
    ///         let handle = builder.launch_node(EthereumNode::default()).await?;
    ///         handle.wait_for_node_exit().await
    ///     })
    ///     .unwrap();
    /// ```
    pub fn with_runner<L, Fut>(self, runner: CliRunner, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
        C: ChainSpecParser<ChainSpec = ChainSpec>,
    {
        let mut app = self.configure();
        app.set_runner(runner);
        app.run(FnLauncher::new::<C, Ext>(async move |builder, ext| launcher(builder, ext).await))
    }

    /// Execute the configured cli command with the provided [`CliRunner`] and
    /// [`CliComponentsBuilder`].
    pub fn with_runner_and_components<N>(
        mut self,
        runner: CliRunner,
        components: impl CliComponentsBuilder<N>,
        launcher: impl AsyncFnOnce(
            WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>,
            Ext,
        ) -> eyre::Result<()>,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<Primitives: NodePrimitives<BlockHeader: HeaderMut>, ChainSpec: Hardforks>,
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        // Add network name if available to the logs dir
        if let Some(chain_spec) = self.command.chain_spec() {
            self.logs.log_file_directory =
                self.logs.log_file_directory.join(chain_spec.chain().to_string());
        }

        // Apply node-specific log defaults before initializing tracing
        if matches!(self.command, Commands::Node(_)) {
            self.logs.apply_node_defaults();
        }

        let _guard = self.init_tracing(&runner, Layers::new())?;

        // Install the prometheus recorder to be sure to record all metrics
        install_prometheus_recorder();

        // Use the shared standalone function to avoid duplication
        run_commands_with::<C, Ext, Rpc, N, SubCmd>(self, runner, components, launcher)
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    ///
    /// If an OTLP endpoint is specified, it will export traces and logs to the configured
    /// collector.
    pub fn init_tracing(
        &mut self,
        runner: &CliRunner,
        mut layers: Layers,
    ) -> eyre::Result<Option<FileWorkerGuard>> {
        let otlp_status = runner.block_on(self.traces.init_otlp_tracing(&mut layers))?;
        let otlp_logs_status = runner.block_on(self.traces.init_otlp_logs(&mut layers))?;

        let guard = self.logs.init_tracing_with_layers(layers)?;
        info!(target: "reth::cli", "Initialized tracing, debug log directory: {}", self.logs.log_file_directory);

        match otlp_status {
            OtlpInitStatus::Started(endpoint) => {
                info!(target: "reth::cli", "Started OTLP {:?} tracing export to {endpoint}", self.traces.protocol);
            }
            OtlpInitStatus::NoFeature => {
                warn!(target: "reth::cli", "Provided OTLP tracing arguments do not have effect, compile with the `otlp` feature")
            }
            OtlpInitStatus::Disabled => {}
        }

        match otlp_logs_status {
            OtlpLogsStatus::Started(endpoint) => {
                info!(target: "reth::cli", "Started OTLP {:?} logs export to {endpoint}", self.traces.protocol);
            }
            OtlpLogsStatus::NoFeature => {
                warn!(target: "reth::cli", "Provided OTLP logs arguments do not have effect, compile with the `otlp-logs` feature")
            }
            OtlpLogsStatus::Disabled => {}
        }

        Ok(guard)
    }
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands<
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    SubCmd: Subcommand + fmt::Debug = NoSubCmd,
> {
    /// Start the node
    #[command(name = "node")]
    Node(Box<node::NodeCommand<C, Ext>>),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(init_cmd::InitCommand<C>),
    /// Initialize the database from a state dump file.
    #[command(name = "init-state")]
    InitState(init_state::InitStateCommand<C>),
    /// This syncs RLP encoded blocks from a file or files.
    #[command(name = "import")]
    Import(import::ImportCommand<C>),
    /// This syncs ERA encoded blocks from a directory.
    #[command(name = "import-era")]
    ImportEra(import_era::ImportEraCommand<C>),
    /// Exports block to era1 files in a specified directory.
    #[command(name = "export-era")]
    ExportEra(export_era::ExportEraCommand<C>),
    /// Dumps genesis block JSON configuration to stdout.
    DumpGenesis(dump_genesis::DumpGenesisCommand<C>),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(Box<db::Command<C>>),
    /// Download public node snapshots
    #[command(name = "download")]
    Download(download::DownloadCommand<C>),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(stage::Command<C>),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(Box<p2p::Command<C>>),
    /// Generate Test Vectors
    #[cfg(feature = "dev")]
    #[command(name = "test-vectors")]
    TestVectors(reth_cli_commands::test_vectors::Command),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config_cmd::Command),
    /// Prune according to the configuration without any limits
    #[command(name = "prune")]
    Prune(prune::PruneCommand<C>),
    /// Re-execute blocks in parallel to verify historical sync correctness.
    #[command(name = "re-execute")]
    ReExecute(re_execute::Command<C>),
    /// Extension subcommands provided by consumers.
    #[command(flatten)]
    Ext(SubCmd),
}

/// A no-op subcommand type for when no extension subcommands are needed.
///
/// This is the default type parameter for `Commands` when consumers don't need
/// to add custom subcommands.
#[derive(Debug, Subcommand)]
pub enum NoSubCmd {}

impl crate::app::ExtendedCommand for NoSubCmd {
    fn execute(self, _runner: CliRunner) -> eyre::Result<()> {
        match self {}
    }
}

impl<C: ChainSpecParser, Ext: clap::Args + fmt::Debug, SubCmd: Subcommand + fmt::Debug>
    Commands<C, Ext, SubCmd>
{
    /// Returns the underlying chain being used for commands
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        match self {
            Self::Node(cmd) => cmd.chain_spec(),
            Self::Init(cmd) => cmd.chain_spec(),
            Self::InitState(cmd) => cmd.chain_spec(),
            Self::Import(cmd) => cmd.chain_spec(),
            Self::ExportEra(cmd) => cmd.chain_spec(),
            Self::ImportEra(cmd) => cmd.chain_spec(),
            Self::DumpGenesis(cmd) => cmd.chain_spec(),
            Self::Db(cmd) => cmd.chain_spec(),
            Self::Download(cmd) => cmd.chain_spec(),
            Self::Stage(cmd) => cmd.chain_spec(),
            Self::P2P(cmd) => cmd.chain_spec(),
            #[cfg(feature = "dev")]
            Self::TestVectors(_) => None,
            Self::Config(_) => None,
            Self::Prune(cmd) => cmd.chain_spec(),
            Self::ReExecute(cmd) => cmd.chain_spec(),
            Self::Ext(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chainspec::SUPPORTED_CHAINS;
    use clap::CommandFactory;
    use reth_chainspec::SEPOLIA;
    use reth_node_core::args::ColorMode;

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

    /// Tests that the log directory is parsed correctly when using the node command. It's
    /// always tied to the specific chain's name.
    #[test]
    fn parse_logs_path_node() {
        let mut reth = Cli::try_parse_args_from(["reth", "node"]).unwrap();
        if let Some(chain_spec) = reth.command.chain_spec() {
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(chain_spec.chain.to_string());
        }
        let log_dir = reth.logs.log_file_directory;
        let end = format!("reth/logs/{}", SUPPORTED_CHAINS[0]);
        assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");

        let mut iter = SUPPORTED_CHAINS.iter();
        iter.next();
        for chain in iter {
            let mut reth = Cli::try_parse_args_from(["reth", "node", "--chain", chain]).unwrap();
            let chain =
                reth.command.chain_spec().map(|c| c.chain.to_string()).unwrap_or(String::new());
            reth.logs.log_file_directory = reth.logs.log_file_directory.join(chain.clone());
            let log_dir = reth.logs.log_file_directory;
            let end = format!("reth/logs/{chain}");
            assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");
        }
    }

    /// Tests that the log directory is parsed correctly when using the init command. It
    /// uses the underlying environment in command to get the chain.
    #[test]
    fn parse_logs_path_init() {
        let mut reth = Cli::try_parse_args_from(["reth", "init"]).unwrap();
        if let Some(chain_spec) = reth.command.chain_spec() {
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(chain_spec.chain.to_string());
        }
        let log_dir = reth.logs.log_file_directory;
        let end = format!("reth/logs/{}", SUPPORTED_CHAINS[0]);
        println!("{log_dir:?}");
        assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");
    }

    /// Tests that the config command does not return any chain spec leading to empty chain id.
    #[test]
    fn parse_empty_logs_path() {
        let mut reth = Cli::try_parse_args_from(["reth", "config"]).unwrap();
        if let Some(chain_spec) = reth.command.chain_spec() {
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(chain_spec.chain.to_string());
        }
        let log_dir = reth.logs.log_file_directory;
        let end = "reth/logs".to_string();
        println!("{log_dir:?}");
        assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");
    }

    #[test]
    fn log_file_max_files_defaults() {
        use reth_node_core::args::LogArgs;

        // Node command without explicit --log.file.max-files should get Some(5) after
        // apply_node_defaults
        let mut cli = Cli::try_parse_args_from(["reth", "node"]).unwrap();
        assert!(cli.logs.log_file_max_files.is_none());
        cli.logs.apply_node_defaults();
        assert_eq!(cli.logs.log_file_max_files, Some(LogArgs::DEFAULT_MAX_LOG_FILES_NODE));

        // Non-node command without explicit --log.file.max-files should be None and
        // effective_log_file_max_files returns 0
        let cli = Cli::try_parse_args_from(["reth", "config"]).unwrap();
        assert!(cli.logs.log_file_max_files.is_none());
        assert_eq!(cli.logs.effective_log_file_max_files(), 0);

        // Explicitly set value should be preserved for node command
        let mut cli =
            Cli::try_parse_args_from(["reth", "node", "--log.file.max-files", "10"]).unwrap();
        assert_eq!(cli.logs.log_file_max_files, Some(10));
        cli.logs.apply_node_defaults();
        assert_eq!(cli.logs.log_file_max_files, Some(10));

        // Explicitly set value should be preserved for non-node command
        let cli =
            Cli::try_parse_args_from(["reth", "config", "--log.file.max-files", "3"]).unwrap();
        assert_eq!(cli.logs.log_file_max_files, Some(3));
        assert_eq!(cli.logs.effective_log_file_max_files(), 3);

        // Setting to 0 explicitly should work
        let cli = Cli::try_parse_args_from(["reth", "node", "--log.file.max-files", "0"]).unwrap();
        assert_eq!(cli.logs.log_file_max_files, Some(0));
        assert_eq!(cli.logs.effective_log_file_max_files(), 0);
    }

    #[test]
    fn parse_env_filter_directives() {
        let temp_dir = tempfile::tempdir().unwrap();

        unsafe { std::env::set_var("RUST_LOG", "info,evm=debug") };
        let reth = Cli::try_parse_args_from([
            "reth",
            "init",
            "--datadir",
            temp_dir.path().to_str().unwrap(),
            "--log.file.filter",
            "debug,net=trace",
        ])
        .unwrap();
        assert!(reth.run(async move |_, _| Ok(())).is_ok());
    }

    #[test]
    fn test_rpc_module_validation() {
        use reth_rpc_server_types::RethRpcModule;

        // Test that standard modules are accepted
        let cli =
            Cli::try_parse_args_from(["reth", "node", "--http.api", "eth,admin,debug"]).unwrap();

        if let Commands::Node(command) = &cli.command {
            if let Some(http_api) = &command.rpc.http_api {
                // Should contain the expected modules
                let modules = http_api.to_selection();
                assert!(modules.contains(&RethRpcModule::Eth));
                assert!(modules.contains(&RethRpcModule::Admin));
                assert!(modules.contains(&RethRpcModule::Debug));
            } else {
                panic!("Expected http.api to be set");
            }
        } else {
            panic!("Expected Node command");
        }

        // Test that unknown modules are parsed as Other variant
        let cli =
            Cli::try_parse_args_from(["reth", "node", "--http.api", "eth,customrpc"]).unwrap();

        if let Commands::Node(command) = &cli.command {
            if let Some(http_api) = &command.rpc.http_api {
                let modules = http_api.to_selection();
                assert!(modules.contains(&RethRpcModule::Eth));
                assert!(modules.contains(&RethRpcModule::Other("customrpc".to_string())));
            } else {
                panic!("Expected http.api to be set");
            }
        } else {
            panic!("Expected Node command");
        }
    }

    #[test]
    fn test_rpc_module_unknown_rejected() {
        use reth_cli_runner::CliRunner;

        // Test that unknown module names are rejected during validation
        let cli =
            Cli::try_parse_args_from(["reth", "node", "--http.api", "unknownmodule"]).unwrap();

        // When we try to run the CLI with validation, it should fail
        let runner = CliRunner::try_default_runtime().unwrap();
        let result = cli.with_runner(runner, |_, _| async { Ok(()) });

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();

        // The error should mention it's an unknown module
        assert!(
            err_msg.contains("Unknown RPC module"),
            "Error should mention unknown module: {}",
            err_msg
        );
        assert!(
            err_msg.contains("'unknownmodule'"),
            "Error should mention the module name: {}",
            err_msg
        );
    }

    #[test]
    fn parse_unwind_chain() {
        let cli = Cli::try_parse_args_from([
            "reth", "stage", "unwind", "--chain", "sepolia", "to-block", "100",
        ])
        .unwrap();
        match cli.command {
            Commands::Stage(cmd) => match cmd.command {
                stage::Subcommands::Unwind(cmd) => {
                    assert_eq!(cmd.chain_spec().unwrap().chain_id(), SEPOLIA.chain_id());
                }
                _ => panic!("Expected Unwind command"),
            },
            _ => panic!("Expected Stage command"),
        };
    }

    #[test]
    fn parse_empty_supported_chains() {
        #[derive(Debug, Clone, Default)]
        struct FileChainSpecParser;

        impl ChainSpecParser for FileChainSpecParser {
            type ChainSpec = ChainSpec;

            const SUPPORTED_CHAINS: &'static [&'static str] = &[];

            fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
                EthereumChainSpecParser::parse(s)
            }
        }

        let cli = Cli::<FileChainSpecParser>::try_parse_from([
            "reth", "stage", "unwind", "--chain", "sepolia", "to-block", "100",
        ])
        .unwrap();
        match cli.command {
            Commands::Stage(cmd) => match cmd.command {
                stage::Subcommands::Unwind(cmd) => {
                    assert_eq!(cmd.chain_spec().unwrap().chain_id(), SEPOLIA.chain_id());
                }
                _ => panic!("Expected Unwind command"),
            },
            _ => panic!("Expected Stage command"),
        };
    }

    #[test]
    fn test_extensible_subcommands() {
        use crate::app::ExtendedCommand;
        use reth_cli_runner::CliRunner;
        use reth_rpc_server_types::DefaultRpcModuleValidator;
        use std::sync::atomic::{AtomicBool, Ordering};

        #[derive(Debug, Subcommand)]
        enum CustomCommands {
            /// A custom hello command
            #[command(name = "hello")]
            Hello {
                /// Name to greet
                #[arg(long)]
                name: String,
            },
            /// Another custom command
            #[command(name = "goodbye")]
            Goodbye,
        }

        static EXECUTED: AtomicBool = AtomicBool::new(false);

        impl ExtendedCommand for CustomCommands {
            fn execute(self, _runner: CliRunner) -> eyre::Result<()> {
                match self {
                    Self::Hello { name } => {
                        assert_eq!(name, "world");
                        EXECUTED.store(true, Ordering::SeqCst);
                        Ok(())
                    }
                    Self::Goodbye => Ok(()),
                }
            }
        }

        // Test parsing the custom "hello" command
        let cli = Cli::<
            EthereumChainSpecParser,
            NoArgs,
            DefaultRpcModuleValidator,
            CustomCommands,
        >::try_parse_from(["reth", "hello", "--name", "world"])
        .unwrap();

        match &cli.command {
            Commands::Ext(CustomCommands::Hello { name }) => {
                assert_eq!(name, "world");
            }
            _ => panic!("Expected Ext(Hello) command"),
        }

        // Test parsing the custom "goodbye" command
        let cli = Cli::<
            EthereumChainSpecParser,
            NoArgs,
            DefaultRpcModuleValidator,
            CustomCommands,
        >::try_parse_from(["reth", "goodbye"])
        .unwrap();

        match &cli.command {
            Commands::Ext(CustomCommands::Goodbye) => {}
            _ => panic!("Expected Ext(Goodbye) command"),
        }

        // Test that built-in commands still work alongside custom ones
        let cli = Cli::<
            EthereumChainSpecParser,
            NoArgs,
            DefaultRpcModuleValidator,
            CustomCommands,
        >::try_parse_from(["reth", "node"])
        .unwrap();

        match &cli.command {
            Commands::Node(_) => {}
            _ => panic!("Expected Node command"),
        }

        // Test executing the custom command
        let cli = Cli::<
            EthereumChainSpecParser,
            NoArgs,
            DefaultRpcModuleValidator,
            CustomCommands,
        >::try_parse_from(["reth", "hello", "--name", "world"])
        .unwrap();

        if let Commands::Ext(cmd) = cli.command {
            let runner = CliRunner::try_default_runtime().unwrap();
            cmd.execute(runner).unwrap();
            assert!(EXECUTED.load(Ordering::SeqCst), "Custom command should have been executed");
        }
    }
}
