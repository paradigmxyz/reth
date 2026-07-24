use crate::{
    interface::{Commands, NoSubCmd},
    Cli,
};
use clap::Subcommand;
use eyre::{eyre, Result};
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec, EthChainSpec, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::{
    common::{CliComponentsBuilder, CliNodeTypes, HeaderMut},
    launcher::{FnLauncher, Launcher},
};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_node_api::NodePrimitives;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_node_core::args::DevArgs;
use reth_node_ethereum::{consensus::EthBeaconConsensus, EthereumNode};
use reth_node_metrics::recorder::install_prometheus_recorder;
use reth_rpc_server_types::RpcModuleValidator;
use reth_tasks::RayonConfig;
use reth_tracing::{Layers, TracingGuards};
use std::{fmt, sync::Arc};

/// A wrapper around a parsed CLI that handles command execution.
#[derive(Debug)]
pub struct CliApp<
    Spec: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    Rpc: RpcModuleValidator,
    SubCmd: Subcommand + fmt::Debug = NoSubCmd,
> {
    cli: Cli<Spec, Ext, Rpc, SubCmd>,
    runner: Option<CliRunner>,
    layers: Option<Layers>,
    guard: Option<TracingGuards>,
}

impl<C, Ext, Rpc, SubCmd> CliApp<C, Ext, Rpc, SubCmd>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    Rpc: RpcModuleValidator,
    SubCmd: ExtendedCommand + Subcommand + fmt::Debug,
{
    pub(crate) fn new(cli: Cli<C, Ext, Rpc, SubCmd>) -> Self {
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
    pub fn run(mut self, launcher: impl Launcher<C, Ext>) -> Result<()>
    where
        C: ChainSpecParser<ChainSpec = ChainSpec>,
    {
        if let Commands::Node(command) = &mut self.cli.command {
            apply_dev_chain_spec_overrides(&mut command.dev, &mut command.chain);
        }

        let jit_args = match &self.cli.command {
            Commands::ReExecute(cmd) => cmd.jit.clone(),
            _ => Default::default(),
        };

        let components = move |spec: Arc<ChainSpec>| {
            let (evm_config, _) =
                reth_node_ethereum::node::build_evm_config(spec.clone(), &jit_args, None)
                    .expect("failed to start revmc JIT backend");
            (evm_config, Arc::new(EthBeaconConsensus::new(spec)))
        };

        self.run_with_components::<EthereumNode>(components, async move |builder, ext| {
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
            WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>,
            Ext,
        ) -> Result<()>,
    ) -> Result<()>
    where
        N: CliNodeTypes<Primitives: NodePrimitives<BlockHeader: HeaderMut>, ChainSpec: Hardforks>,
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        let runner = match self.runner.take() {
            Some(runner) => runner,
            None => {
                let runtime_config = match &self.cli.command {
                    Commands::Node(command) => {
                        reth_tasks::RuntimeConfig::default().with_rayon(RayonConfig {
                            reserved_cpu_cores: command.engine.reserved_cpu_cores,
                            proof_storage_worker_threads: command.engine.storage_worker_count,
                            proof_account_worker_threads: command.engine.account_worker_count,
                            prewarming_threads: command.engine.prewarming_threads,
                            ..Default::default()
                        })
                    }
                    _ => reth_tasks::RuntimeConfig::default(),
                };
                CliRunner::try_with_runtime_config(runtime_config)?
            }
        };

        // Add network name if available to the logs dir
        if let Some(chain_spec) = self.cli.command.chain_spec() {
            self.cli.logs.log_file_directory =
                self.cli.logs.log_file_directory.join(chain_spec.chain().to_string());
        }

        // Apply node-specific log defaults before initializing tracing
        if matches!(self.cli.command, Commands::Node(_)) {
            self.cli.logs.apply_node_defaults();
        }

        self.init_tracing(&runner)?;

        // Deprioritize background threads spawned by tracing/OTel libraries.
        reth_tasks::utils::deprioritize_background_threads();

        // Install the prometheus recorder to be sure to record all metrics
        install_prometheus_recorder();

        run_commands_with::<C, Ext, Rpc, N, SubCmd>(self.cli, runner, components, launcher)
    }

    /// Initializes tracing with the configured options.
    ///
    /// See [`Cli::init_tracing`] for more information.
    pub fn init_tracing(&mut self, runner: &CliRunner) -> Result<()> {
        if let Some(layers) = self.layers.take() {
            self.guard = Some(self.cli.init_tracing(runner, layers)?);
        }

        Ok(())
    }
}

/// Run CLI commands with the provided runner, components and launcher.
/// This is the shared implementation used by both `CliApp` and Cli methods.
pub(crate) fn run_commands_with<C, Ext, Rpc, N, SubCmd>(
    cli: Cli<C, Ext, Rpc, SubCmd>,
    runner: CliRunner,
    components: impl CliComponentsBuilder<N>,
    launcher: impl AsyncFnOnce(
        WithLaunchContext<NodeBuilder<DatabaseEnv, C::ChainSpec>>,
        Ext,
    ) -> Result<()>,
) -> Result<()>
where
    C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    Ext: clap::Args + fmt::Debug,
    Rpc: RpcModuleValidator,
    N: CliNodeTypes<Primitives: NodePrimitives<BlockHeader: HeaderMut>, ChainSpec: Hardforks>,
    SubCmd: ExtendedCommand + Subcommand + fmt::Debug,
{
    let rt = runner.runtime();

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
        Commands::Init(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>(rt)),
        Commands::InitState(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>(rt)),
        Commands::Import(command) => {
            runner.run_blocking_until_ctrl_c(command.execute::<N, _>(components, rt))
        }
        Commands::ImportEra(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>(rt)),
        Commands::ExportEra(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>(rt)),
        Commands::DumpGenesis(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::Db(command) => {
            runner.run_blocking_command_until_exit(|ctx| command.execute::<N>(ctx))
        }
        Commands::Download(command) => runner.run_blocking_until_ctrl_c(command.execute::<N>()),
        Commands::SnapshotManifest(command) => command.execute(),
        Commands::Stage(command) => {
            runner.run_command_until_exit(|ctx| command.execute::<N, _>(ctx, components))
        }
        Commands::P2P(command) => runner.run_until_ctrl_c(command.execute::<N>()),
        Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Prune(command) => runner.run_command_until_exit(|ctx| command.execute::<N>(ctx)),
        #[cfg(feature = "dev")]
        Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::ReExecute(command) => {
            runner.run_until_ctrl_c(command.execute::<N>(components, rt))
        }
        Commands::Ext(command) => command.execute(runner),
    }
}

/// A trait for extension subcommands that can be added to the CLI.
///
/// Consumers implement this trait for their custom subcommands to define
/// how they should be executed.
pub trait ExtendedCommand {
    /// Execute the extension command with the provided CLI runner.
    fn execute(self, runner: CliRunner) -> Result<()>;
}

/// Applies dev-mode overrides to the chain spec before the node is built.
///
/// With a bare `--dev.constant-base-fee`, base fee adjustment is disabled by setting the
/// EIP-1559 max change denominator to zero: `alloy_eips::eip1559::calc_next_block_base_fee`
/// then returns the parent's base fee unchanged. With an explicit value
/// (`--dev.constant-base-fee=<WEI>`), the base fee of every mined block is pinned to that
/// value via [`BaseFeeParamsKind::TestingOverride`]. The payload builder, consensus
/// validation and the transaction pool all derive the next base fee from the chain spec, so
/// locally mined blocks stay self-consistent either way.
///
/// The flag is taken out of `dev` once applied: entry points that never call this (e.g. the
/// `run_with_components` family) leave it set, which the dev-mode launcher detects to warn
/// that the flag was ignored.
fn apply_dev_chain_spec_overrides(dev: &mut DevArgs, chain: &mut Arc<ChainSpec>) {
    if !dev.dev {
        return;
    }
    // The chain's own elasticity is preserved in both arms: it still feeds consumers that
    // read the adjustment params directly, such as gas limit validation at the London
    // transition block.
    let elasticity = chain.base_fee_params_at_timestamp(u64::MAX).elasticity_multiplier;
    match dev.constant_base_fee.take() {
        // freeze at the parent (i.e. chain tip) value
        Some(None) => {
            Arc::make_mut(chain).base_fee_params =
                BaseFeeParamsKind::Constant(BaseFeeParams::new(0, elasticity));
        }
        // pin to an explicit value
        Some(Some(base_fee)) => {
            Arc::make_mut(chain).base_fee_params = BaseFeeParamsKind::TestingOverride {
                base_fee,
                params: BaseFeeParams::new(0, elasticity),
            };
        }
        None => {}
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
    fn dev_constant_base_fee_freezes_base_fee() {
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from([
            "reth",
            "node",
            "--chain",
            "dev",
            "--dev",
            "--dev.constant-base-fee",
        ])
        .unwrap();
        let Commands::Node(mut command) = cli.command else { panic!("expected node command") };

        let parent = command.chain.genesis_header().clone();
        let parent_base_fee = parent.base_fee_per_gas.unwrap();

        // sanity: with default params the base fee of an empty parent block decreases
        let next = command.chain.next_block_base_fee(&parent, parent.timestamp + 1).unwrap();
        assert!(next < parent_base_fee);

        apply_dev_chain_spec_overrides(&mut command.dev, &mut command.chain);
        let next = command.chain.next_block_base_fee(&parent, parent.timestamp + 1).unwrap();
        assert_eq!(next, parent_base_fee);
        // consumed once applied so unapplied uses can be detected
        assert!(command.dev.constant_base_fee.is_none());
    }

    #[test]
    fn dev_constant_base_fee_pins_base_fee() {
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from([
            "reth",
            "node",
            "--chain",
            "dev",
            "--dev",
            "--dev.constant-base-fee=7",
        ])
        .unwrap();
        let Commands::Node(mut command) = cli.command else { panic!("expected node command") };

        let parent = command.chain.genesis_header().clone();
        assert_ne!(parent.base_fee_per_gas, Some(7));

        let elasticity_before =
            command.chain.base_fee_params_at_timestamp(u64::MAX).elasticity_multiplier;
        apply_dev_chain_spec_overrides(&mut command.dev, &mut command.chain);
        let next = command.chain.next_block_base_fee(&parent, parent.timestamp + 1).unwrap();
        assert_eq!(next, 7);
        // the chain's elasticity survives the override, and the reported params freeze the
        // base fee for consumers that bypass `ChainSpec::next_block_base_fee`
        let params = command.chain.base_fee_params_at_timestamp(u64::MAX);
        assert_eq!(params.elasticity_multiplier, elasticity_before);
        assert_eq!(params.max_change_denominator, 0);
    }

    #[test]
    fn dev_overrides_noop_without_flag() {
        let cli = Cli::<EthereumChainSpecParser, NoArgs>::try_parse_from(["reth", "node", "--dev"])
            .unwrap();
        let Commands::Node(mut command) = cli.command else { panic!("expected node command") };

        let params_before = command.chain.base_fee_params_at_timestamp(u64::MAX);
        apply_dev_chain_spec_overrides(&mut command.dev, &mut command.chain);
        assert_eq!(command.chain.base_fee_params_at_timestamp(u64::MAX), params_before);
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
