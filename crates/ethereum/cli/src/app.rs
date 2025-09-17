use crate::{interface::Commands, Cli};
use eyre::{eyre, Result};
use reth_chainspec::{ChainSpec, EthChainSpec, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::{
    common::{CliComponentsBuilder, CliHeader, CliNodeTypes},
    launcher::{FnLauncher, Launcher},
};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_node_api::NodePrimitives;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_node_ethereum::{consensus::EthBeaconConsensus, EthEvmConfig, EthereumNode};
use reth_node_metrics::recorder::install_prometheus_recorder;
use reth_rpc_server_types::RpcModuleValidator;
use reth_tracing::{FileWorkerGuard, Layers};
use std::{fmt, sync::Arc};
use tracing::info;

/// A wrapper around a parsed CLI that handles command execution.
#[derive(Debug)]
pub struct CliApp<Spec: ChainSpecParser, Ext: clap::Args + fmt::Debug, Rpc: RpcModuleValidator> {
    cli: Cli<Spec, Ext, Rpc>,
    runner: Option<CliRunner>,
    layers: Option<Layers>,
    guard: Option<FileWorkerGuard>,
}

impl<C, Ext, Rpc> CliApp<C, Ext, Rpc>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    Rpc: RpcModuleValidator,
{
    pub(crate) fn new(cli: Cli<C, Ext, Rpc>) -> Self {
        Self { cli, runner: None, layers: Some(Layers::new()), guard: None }
    }

    /// Sets the runner for the CLI commander.
    ///
    /// This replaces any existing runner with the provided one.
    pub fn set_runner(&mut self, runner: CliRunner) {
        self.runner = Some(runner);
    }

    /// Access to tracing layers.
    ///
    /// Returns a mutable reference to the tracing layers, or error
    /// if tracing initialized and layers have detached already.
    pub fn access_tracing_layers(&mut self) -> Result<&mut Layers> {
        self.layers.as_mut().ok_or_else(|| eyre!("Tracing already initialized"))
    }

    /// Execute the configured cli command.
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](reth_cli_commands::node::NodeCommand).
    pub fn run(self, launcher: impl Launcher<C, Ext>) -> Result<()>
    where
        C: ChainSpecParser<ChainSpec = ChainSpec>,
    {
        let components = |spec: Arc<ChainSpec>| {
            (EthEvmConfig::ethereum(spec.clone()), Arc::new(EthBeaconConsensus::new(spec)))
        };

        self.run_with_components::<EthereumNode>(components, |builder, ext| async move {
            launcher.entrypoint(builder, ext).await
        })
    }

    /// Execute the configured cli command with the provided [`CliComponentsBuilder`].
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [`NodeCommand`](reth_cli_commands::node::NodeCommand) and allows providing custom
    /// components.
    pub fn run_with_components<N>(
        mut self,
        components: impl CliComponentsBuilder<N>,
        launcher: impl AsyncFnOnce(
            WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
            Ext,
        ) -> Result<()>,
    ) -> Result<()>
    where
        N: CliNodeTypes<
            Primitives: NodePrimitives<BlockHeader: CliHeader>,
            ChainSpec: Hardforks + EthChainSpec,
        >,
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        let runner = match self.runner.take() {
            Some(runner) => runner,
            None => CliRunner::try_default_runtime()?,
        };

        // Add network name if available to the logs dir
        if let Some(chain_spec) = self.cli.command.chain_spec() {
            self.cli.logs.log_file_directory =
                self.cli.logs.log_file_directory.join(chain_spec.chain().to_string());
        }

        self.init_tracing()?;
        // Install the prometheus recorder to be sure to record all metrics
        let _ = install_prometheus_recorder();

        run_commands_with::<C, Ext, Rpc, N>(self.cli, runner, components, launcher)
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function stores guard to the struct.
    pub fn init_tracing(&mut self) -> Result<()> {
        if self.guard.is_none() {
            let layers = self.layers.take().unwrap_or_default();
            self.guard = self.cli.logs.init_tracing_with_layers(layers)?;
            info!(target: "reth::cli", "Initialized tracing, debug log directory: {}", self.cli.logs.log_file_directory);
        }
        Ok(())
    }
}

/// Run CLI commands with the provided runner, components and launcher.
/// This is the shared implementation used by both `CliApp` and Cli methods.
pub(crate) fn run_commands_with<C, Ext, Rpc, N>(
    cli: Cli<C, Ext, Rpc>,
    runner: CliRunner,
    components: impl CliComponentsBuilder<N>,
    launcher: impl AsyncFnOnce(
        WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        Ext,
    ) -> Result<()>,
) -> Result<()>
where
    C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    Ext: clap::Args + fmt::Debug,
    Rpc: RpcModuleValidator,
    N: CliNodeTypes<Primitives: NodePrimitives<BlockHeader: CliHeader>, ChainSpec: Hardforks>,
{
    match cli.command {
        Commands::Node(command) => {
            // Validate RPC modules using the configured validator
            if let Some(http_api) = &command.rpc.http_api {
                Rpc::validate_selection(http_api, "http.api").map_err(|e| eyre!("{e}"))?;
            }
            if let Some(ws_api) = &command.rpc.ws_api {
                Rpc::validate_selection(ws_api, "ws.api").map_err(|e| eyre!("{e}"))?;
            }

            runner.run_command_until_exit(|ctx| {
                command.execute(ctx, FnLauncher::new::<C, Ext>(launcher))
            })
        }
        Commands::Init(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::InitState(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::Import(command) => {
            runner.run_blocking_until_ctrl_c(command.execute::<N, _>(components))
        }
        Commands::ImportEra(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::ExportEra(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::DumpGenesis(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::Db(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::Download(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::Stage(command) => {
            runner.run_command_until_exit(|ctx| command.execute::<N, _>(ctx, components))
        }
        Commands::P2P(command) => runner.run_until_ctrl_c(command.execute::<N>()),
        Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Prune(command) => runner.run_until_ctrl_c(command.execute::<N>()),
        #[cfg(feature = "dev")]
        Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::ReExecute(command) => runner.run_until_ctrl_c(command.execute::<N>(components)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chainspec::EthereumChainSpecParser;
    use clap::Parser;
    use reth_cli_commands::node::NoArgs;

    #[test]
    fn test_cli_app_creation() {
        let args = vec!["reth", "config"];
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from(args).unwrap();
        let app = cli.configure();

        // Verify app is created correctly
        assert!(app.runner.is_none());
        assert!(app.layers.is_some());
        assert!(app.guard.is_none());
    }

    #[test]
    fn test_set_runner() {
        let args = vec!["reth", "config"];
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from(args).unwrap();
        let mut app = cli.configure();

        // Create and set a runner
        if let Ok(runner) = CliRunner::try_default_runtime() {
            app.set_runner(runner);
            assert!(app.runner.is_some());
        }
    }

    #[test]
    fn test_access_tracing_layers() {
        let args = vec!["reth", "config"];
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from(args).unwrap();
        let mut app = cli.configure();

        // Should be able to access layers before initialization
        assert!(app.access_tracing_layers().is_ok());

        // After taking layers (simulating initialization), access should error
        app.layers = None;
        assert!(app.access_tracing_layers().is_err());
    }
}
