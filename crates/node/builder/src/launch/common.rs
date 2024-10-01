//! Helper types that can be used by launchers.

use std::{sync::Arc, thread::available_parallelism};

use alloy_primitives::{BlockNumber, B256};
use eyre::{Context, OptionExt};
use rayon::ThreadPoolBuilder;
use reth_auto_seal_consensus::MiningMode;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_chainspec::{Chain, EthChainSpec, EthereumHardforks};
use reth_config::{config::EtlConfig, PruneConfig};
use reth_consensus::Consensus;
use reth_db_api::database::Database;
use reth_db_common::init::{init_genesis, InitDatabaseError};
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_engine_tree::tree::{InvalidBlockHook, InvalidBlockHooks, NoopInvalidBlockHook};
use reth_evm::noop::NoopBlockExecutorProvider;
use reth_fs_util as fs;
use reth_invalid_block_hooks::InvalidBlockWitnessHook;
use reth_network_p2p::headers::client::HeadersClient;
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithDB};
use reth_node_core::{
    args::InvalidBlockHookType,
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
    version::{
        BUILD_PROFILE_NAME, CARGO_PKG_VERSION, VERGEN_BUILD_TIMESTAMP, VERGEN_CARGO_FEATURES,
        VERGEN_CARGO_TARGET_TRIPLE, VERGEN_GIT_SHA,
    },
};
use reth_node_metrics::{
    chain::ChainSpecInfo,
    hooks::Hooks,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use reth_primitives::Head;
use reth_provider::{
    providers::{BlockchainProvider, BlockchainProvider2, ProviderNodeTypes, StaticFileProvider},
    BlockHashReader, CanonStateNotificationSender, ChainSpecProvider, ProviderFactory,
    ProviderResult, StageCheckpointReader, StateProviderFactory, StaticFileProviderFactory,
    TreeViewer,
};
use reth_prune::{PruneModes, PrunerBuilder};
use reth_rpc_api::clients::EthApiClient;
use reth_rpc_builder::config::RethRpcServerConfig;
use reth_rpc_layer::JwtSecret;
use reth_stages::{sets::DefaultStages, MetricEvent, PipelineBuilder, PipelineTarget, StageId};
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, error, info, warn};
use tokio::sync::{
    mpsc::{unbounded_channel, Receiver, UnboundedSender},
    oneshot, watch,
};

use crate::{
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::OnComponentInitializedHook,
    BuilderContext, NodeAdapter,
};

/// Allows to set a tree viewer for a configured blockchain provider.
// TODO: remove this helper trait once the engine revamp is done, the new
// blockchain provider won't require a TreeViewer.
// https://github.com/paradigmxyz/reth/issues/8742
pub trait WithTree {
    /// Setter for tree viewer.
    fn set_tree(self, tree: Arc<dyn TreeViewer>) -> Self;
}

impl<N: NodeTypesWithDB> WithTree for BlockchainProvider<N> {
    fn set_tree(self, tree: Arc<dyn TreeViewer>) -> Self {
        self.with_tree(tree)
    }
}

impl<N: NodeTypesWithDB> WithTree for BlockchainProvider2<N> {
    fn set_tree(self, _tree: Arc<dyn TreeViewer>) -> Self {
        self
    }
}

/// Reusable setup for launching a node.
///
/// This provides commonly used boilerplate for launching a node.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The data directory for the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl LaunchContext {
    /// Create a new instance of the default node launcher.
    pub const fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { task_executor, data_dir }
    }

    /// Attaches a database to the launch context.
    pub const fn with<DB>(self, database: DB) -> LaunchContextWith<DB> {
        LaunchContextWith { inner: self, attachment: database }
    }

    /// Loads the reth config with the configured `data_dir` and overrides settings according to the
    /// `config`.
    ///
    /// Attaches both the `NodeConfig` and the loaded `reth.toml` config to the launch context.
    pub fn with_loaded_toml_config<ChainSpec: EthChainSpec>(
        self,
        config: NodeConfig<ChainSpec>,
    ) -> eyre::Result<LaunchContextWith<WithConfigs<ChainSpec>>> {
        let toml_config = self.load_toml_config(&config)?;
        Ok(self.with(WithConfigs { config, toml_config }))
    }

    /// Loads the reth config with the configured `data_dir` and overrides settings according to the
    /// `config`.
    ///
    /// This is async because the trusted peers may have to be resolved.
    pub fn load_toml_config<ChainSpec: EthChainSpec>(
        &self,
        config: &NodeConfig<ChainSpec>,
    ) -> eyre::Result<reth_config::Config> {
        let config_path = config.config.clone().unwrap_or_else(|| self.data_dir.config());

        let mut toml_config = reth_config::Config::from_path(&config_path)
            .wrap_err_with(|| format!("Could not load config file {config_path:?}"))?;

        Self::save_pruning_config_if_full_node(&mut toml_config, config, &config_path)?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        toml_config.peers.trusted_nodes_only = config.network.trusted_only;

        Ok(toml_config)
    }

    /// Save prune config to the toml file if node is a full node.
    fn save_pruning_config_if_full_node<ChainSpec: EthChainSpec>(
        reth_config: &mut reth_config::Config,
        config: &NodeConfig<ChainSpec>,
        config_path: impl AsRef<std::path::Path>,
    ) -> eyre::Result<()> {
        if reth_config.prune.is_none() {
            if let Some(prune_config) = config.prune_config() {
                reth_config.update_prune_config(prune_config);
                info!(target: "reth::cli", "Saving prune config to toml file");
                reth_config.save(config_path.as_ref())?;
            }
        } else if config.prune_config().is_none() {
            warn!(target: "reth::cli", "Prune configs present in config file but --full not provided. Running as a Full node");
        }
        Ok(())
    }

    /// Convenience function to [`Self::configure_globals`]
    pub fn with_configured_globals(self) -> Self {
        self.configure_globals();
        self
    }

    /// Configure global settings this includes:
    ///
    /// - Raising the file descriptor limit
    /// - Configuring the global rayon thread pool
    pub fn configure_globals(&self) {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        match fdlimit::raise_fd_limit() {
            Ok(fdlimit::Outcome::LimitRaised { from, to }) => {
                debug!(from, to, "Raised file descriptor limit");
            }
            Ok(fdlimit::Outcome::Unsupported) => {}
            Err(err) => warn!(%err, "Failed to raise file descriptor limit"),
        }

        // Limit the global rayon thread pool, reserving 1 core for the rest of the system.
        // If the system only has 1 core the pool will use it.
        let num_threads =
            available_parallelism().map_or(0, |num| num.get().saturating_sub(1).max(1));
        if let Err(err) = ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|i| format!("reth-rayon-{i}"))
            .build_global()
        {
            error!(%err, "Failed to build global thread pool")
        }
    }
}

/// A [`LaunchContext`] along with an additional value.
///
/// This can be used to sequentially attach additional values to the type during the launch process.
///
/// The type provides common boilerplate for launching a node depending on the additional value.
#[derive(Debug, Clone)]
pub struct LaunchContextWith<T> {
    /// The wrapped launch context.
    pub inner: LaunchContext,
    /// The additional attached value.
    pub attachment: T,
}

impl<T> LaunchContextWith<T> {
    /// Configure global settings this includes:
    ///
    /// - Raising the file descriptor limit
    /// - Configuring the global rayon thread pool
    pub fn configure_globals(&self) {
        self.inner.configure_globals();
    }

    /// Returns the data directory.
    pub const fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.inner.data_dir
    }

    /// Returns the task executor.
    pub const fn task_executor(&self) -> &TaskExecutor {
        &self.inner.task_executor
    }

    /// Attaches another value to the launch context.
    pub fn attach<A>(self, attachment: A) -> LaunchContextWith<Attached<T, A>> {
        LaunchContextWith {
            inner: self.inner,
            attachment: Attached::new(self.attachment, attachment),
        }
    }

    /// Consumes the type and calls a function with a reference to the context.
    // Returns the context again
    pub fn inspect<F>(self, f: F) -> Self
    where
        F: FnOnce(&Self),
    {
        f(&self);
        self
    }
}

impl<ChainSpec> LaunchContextWith<WithConfigs<ChainSpec>> {
    /// Resolves the trusted peers and adds them to the toml config.
    pub async fn with_resolved_peers(mut self) -> eyre::Result<Self> {
        if !self.attachment.config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");

            self.attachment
                .toml_config
                .peers
                .trusted_nodes
                .extend(self.attachment.config.network.trusted_peers.clone());
        }
        Ok(self)
    }
}

impl<L, R> LaunchContextWith<Attached<L, R>> {
    /// Get a reference to the left value.
    pub const fn left(&self) -> &L {
        &self.attachment.left
    }

    /// Get a reference to the right value.
    pub const fn right(&self) -> &R {
        &self.attachment.right
    }

    /// Get a mutable reference to the right value.
    pub fn left_mut(&mut self) -> &mut L {
        &mut self.attachment.left
    }

    /// Get a mutable reference to the right value.
    pub fn right_mut(&mut self) -> &mut R {
        &mut self.attachment.right
    }
}
impl<R, ChainSpec: EthChainSpec> LaunchContextWith<Attached<WithConfigs<ChainSpec>, R>> {
    /// Adjust certain settings in the config to make sure they are set correctly
    ///
    /// This includes:
    /// - Making sure the ETL dir is set to the datadir
    /// - RPC settings are adjusted to the correct port
    pub fn with_adjusted_configs(self) -> Self {
        self.ensure_etl_datadir().with_adjusted_instance_ports()
    }

    /// Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
    pub fn ensure_etl_datadir(mut self) -> Self {
        if self.toml_config_mut().stages.etl.dir.is_none() {
            self.toml_config_mut().stages.etl.dir =
                Some(EtlConfig::from_datadir(self.data_dir().data_dir()))
        }

        self
    }

    /// Change rpc port numbers based on the instance number.
    pub fn with_adjusted_instance_ports(mut self) -> Self {
        self.node_config_mut().adjust_instance_ports();
        self
    }

    /// Returns the container for all config types
    pub const fn configs(&self) -> &WithConfigs<ChainSpec> {
        self.attachment.left()
    }

    /// Returns the attached [`NodeConfig`].
    pub const fn node_config(&self) -> &NodeConfig<ChainSpec> {
        &self.left().config
    }

    /// Returns the attached [`NodeConfig`].
    pub fn node_config_mut(&mut self) -> &mut NodeConfig<ChainSpec> {
        &mut self.left_mut().config
    }

    /// Returns the attached toml config [`reth_config::Config`].
    pub const fn toml_config(&self) -> &reth_config::Config {
        &self.left().toml_config
    }

    /// Returns the attached toml config [`reth_config::Config`].
    pub fn toml_config_mut(&mut self) -> &mut reth_config::Config {
        &mut self.left_mut().toml_config
    }

    /// Returns the configured chain spec.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.node_config().chain.clone()
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> B256 {
        self.node_config().chain.genesis_hash()
    }

    /// Returns the chain identifier of the node.
    pub fn chain_id(&self) -> Chain {
        self.node_config().chain.chain()
    }

    /// Returns true if the node is configured as --dev
    pub const fn is_dev(&self) -> bool {
        self.node_config().dev.dev
    }

    /// Returns the configured [`PruneConfig`]
    /// Any configuration set in CLI will take precedence over those set in toml
    pub fn prune_config(&self) -> Option<PruneConfig> {
        let Some(mut node_prune_config) = self.node_config().prune_config() else {
            // No CLI config is set, use the toml config.
            return self.toml_config().prune.clone();
        };

        // Otherwise, use the CLI configuration and merge with toml config.
        node_prune_config.merge(self.toml_config().prune.clone());
        Some(node_prune_config)
    }

    /// Returns the configured [`PruneModes`], returning the default if no config was available.
    pub fn prune_modes(&self) -> PruneModes {
        self.prune_config().map(|config| config.segments).unwrap_or_default()
    }

    /// Returns an initialized [`PrunerBuilder`] based on the configured [`PruneConfig`]
    pub fn pruner_builder(&self) -> PrunerBuilder {
        PrunerBuilder::new(self.prune_config().unwrap_or_default())
            .delete_limit(self.chain_spec().prune_delete_limit())
            .timeout(PrunerBuilder::DEFAULT_TIMEOUT)
    }

    /// Loads the JWT secret for the engine API
    pub fn auth_jwt_secret(&self) -> eyre::Result<JwtSecret> {
        let default_jwt_path = self.data_dir().jwt();
        let secret = self.node_config().rpc.auth_jwt_secret(default_jwt_path)?;
        Ok(secret)
    }

    /// Returns the [`MiningMode`] intended for --dev mode.
    pub fn dev_mining_mode(&self, pending_transactions_listener: Receiver<B256>) -> MiningMode {
        if let Some(interval) = self.node_config().dev.block_time {
            MiningMode::interval(interval)
        } else if let Some(max_transactions) = self.node_config().dev.block_max_transactions {
            MiningMode::instant(max_transactions, pending_transactions_listener)
        } else {
            MiningMode::instant(1, pending_transactions_listener)
        }
    }
}

impl<DB, ChainSpec> LaunchContextWith<Attached<WithConfigs<ChainSpec>, DB>>
where
    DB: Database + Clone + 'static,
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
{
    /// Returns the [`ProviderFactory`] for the attached storage after executing a consistent check
    /// between the database and static files. **It may execute a pipeline unwind if it fails this
    /// check.**
    pub async fn create_provider_factory<N: NodeTypesWithDB<DB = DB, ChainSpec = ChainSpec>>(
        &self,
    ) -> eyre::Result<ProviderFactory<N>> {
        let factory = ProviderFactory::new(
            self.right().clone(),
            self.chain_spec(),
            StaticFileProvider::read_write(self.data_dir().static_files())?,
        )
        .with_prune_modes(self.prune_modes())
        .with_static_files_metrics();

        let has_receipt_pruning =
            self.toml_config().prune.as_ref().map_or(false, |a| a.has_receipts_pruning());

        // Check for consistency between database and static files. If it fails, it unwinds to
        // the first block that's consistent between database and static files.
        if let Some(unwind_target) = factory
            .static_file_provider()
            .check_consistency(&factory.provider()?, has_receipt_pruning)?
        {
            // Highly unlikely to happen, and given its destructive nature, it's better to panic
            // instead.
            assert_ne!(unwind_target, PipelineTarget::Unwind(0), "A static file <> database inconsistency was found that would trigger an unwind to block 0");

            info!(target: "reth::cli", unwind_target = %unwind_target, "Executing an unwind after a failed storage consistency check.");

            let (_tip_tx, tip_rx) = watch::channel(B256::ZERO);

            // Builds an unwind-only pipeline
            let pipeline = PipelineBuilder::default()
                .add_stages(DefaultStages::new(
                    factory.clone(),
                    tip_rx,
                    Arc::new(EthBeaconConsensus::new(self.chain_spec())),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    NoopBlockExecutorProvider::default(),
                    self.toml_config().stages.clone(),
                    self.prune_modes(),
                ))
                .build(
                    factory.clone(),
                    StaticFileProducer::new(factory.clone(), self.prune_modes()),
                );

            // Unwinds to block
            let (tx, rx) = oneshot::channel();

            // Pipeline should be run as blocking and panic if it fails.
            self.task_executor().spawn_critical_blocking(
                "pipeline task",
                Box::pin(async move {
                    let (_, result) = pipeline.run_as_fut(Some(unwind_target)).await;
                    let _ = tx.send(result);
                }),
            );
            rx.await??;
        }

        Ok(factory)
    }

    /// Creates a new [`ProviderFactory`] and attaches it to the launch context.
    pub async fn with_provider_factory<N: NodeTypesWithDB<DB = DB, ChainSpec = ChainSpec>>(
        self,
    ) -> eyre::Result<LaunchContextWith<Attached<WithConfigs<ChainSpec>, ProviderFactory<N>>>> {
        let factory = self.create_provider_factory().await?;
        let ctx = LaunchContextWith {
            inner: self.inner,
            attachment: self.attachment.map_right(|_| factory),
        };

        Ok(ctx)
    }
}

impl<T> LaunchContextWith<Attached<WithConfigs<T::ChainSpec>, ProviderFactory<T>>>
where
    T: NodeTypesWithDB<ChainSpec: EthereumHardforks + EthChainSpec>,
{
    /// Returns access to the underlying database.
    pub const fn database(&self) -> &T::DB {
        self.right().db_ref()
    }

    /// Returns the configured `ProviderFactory`.
    pub const fn provider_factory(&self) -> &ProviderFactory<T> {
        self.right()
    }

    /// Returns the static file provider to interact with the static files.
    pub fn static_file_provider(&self) -> StaticFileProvider {
        self.right().static_file_provider()
    }

    /// This launches the prometheus endpoint.
    ///
    /// Convenience function to [`Self::start_prometheus_endpoint`]
    pub async fn with_prometheus_server(self) -> eyre::Result<Self> {
        self.start_prometheus_endpoint().await?;
        Ok(self)
    }

    /// Starts the prometheus endpoint.
    pub async fn start_prometheus_endpoint(&self) -> eyre::Result<()> {
        let listen_addr = self.node_config().metrics;
        if let Some(addr) = listen_addr {
            info!(target: "reth::cli", "Starting metrics endpoint at {}", addr);
            let config = MetricServerConfig::new(
                addr,
                VersionInfo {
                    version: CARGO_PKG_VERSION,
                    build_timestamp: VERGEN_BUILD_TIMESTAMP,
                    cargo_features: VERGEN_CARGO_FEATURES,
                    git_sha: VERGEN_GIT_SHA,
                    target_triple: VERGEN_CARGO_TARGET_TRIPLE,
                    build_profile: BUILD_PROFILE_NAME,
                },
                ChainSpecInfo { name: self.left().config.chain.chain().to_string() },
                self.task_executor().clone(),
                Hooks::new(self.database().clone(), self.static_file_provider()),
            );

            MetricServer::new(config).serve().await?;
        }

        Ok(())
    }

    /// Convenience function to [`Self::init_genesis`]
    pub fn with_genesis(self) -> Result<Self, InitDatabaseError> {
        init_genesis(self.provider_factory())?;
        Ok(self)
    }

    /// Write the genesis block and state if it has not already been written
    pub fn init_genesis(&self) -> Result<B256, InitDatabaseError> {
        init_genesis(self.provider_factory())
    }

    /// Creates a new `WithMeteredProvider` container and attaches it to the
    /// launch context.
    ///
    /// This spawns a metrics task that listens for metrics related events and updates metrics for
    /// prometheus.
    pub fn with_metrics_task(
        self,
    ) -> LaunchContextWith<Attached<WithConfigs<T::ChainSpec>, WithMeteredProvider<T>>> {
        let (metrics_sender, metrics_receiver) = unbounded_channel();

        let with_metrics =
            WithMeteredProvider { provider_factory: self.right().clone(), metrics_sender };

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let sync_metrics_listener = reth_stages::MetricsListener::new(metrics_receiver);
        self.task_executor().spawn_critical("stages metrics listener task", sync_metrics_listener);

        LaunchContextWith {
            inner: self.inner,
            attachment: self.attachment.map_right(|_| with_metrics),
        }
    }
}

impl<N> LaunchContextWith<Attached<WithConfigs<N::ChainSpec>, WithMeteredProvider<N>>>
where
    N: NodeTypesWithDB,
{
    /// Returns the configured `ProviderFactory`.
    const fn provider_factory(&self) -> &ProviderFactory<N> {
        &self.right().provider_factory
    }

    /// Returns the metrics sender.
    fn sync_metrics_tx(&self) -> UnboundedSender<MetricEvent> {
        self.right().metrics_sender.clone()
    }

    /// Creates a `BlockchainProvider` and attaches it to the launch context.
    #[allow(clippy::complexity)]
    pub fn with_blockchain_db<T, F>(
        self,
        create_blockchain_provider: F,
        tree_config: BlockchainTreeConfig,
        canon_state_notification_sender: CanonStateNotificationSender,
    ) -> eyre::Result<LaunchContextWith<Attached<WithConfigs<N::ChainSpec>, WithMeteredProviders<T>>>>
    where
        T: FullNodeTypes<Types = N>,
        F: FnOnce(ProviderFactory<N>) -> eyre::Result<T::Provider>,
    {
        let blockchain_db = create_blockchain_provider(self.provider_factory().clone())?;

        let metered_providers = WithMeteredProviders {
            db_provider_container: WithMeteredProvider {
                provider_factory: self.provider_factory().clone(),
                metrics_sender: self.sync_metrics_tx(),
            },
            blockchain_db,
            tree_config,
            canon_state_notification_sender,
        };

        let ctx = LaunchContextWith {
            inner: self.inner,
            attachment: self.attachment.map_right(|_| metered_providers),
        };

        Ok(ctx)
    }
}

impl<T>
    LaunchContextWith<
        Attached<WithConfigs<<T::Types as NodeTypes>::ChainSpec>, WithMeteredProviders<T>>,
    >
where
    T: FullNodeTypes<Types: ProviderNodeTypes, Provider: WithTree>,
{
    /// Returns access to the underlying database.
    pub const fn database(&self) -> &<T::Types as NodeTypesWithDB>::DB {
        self.provider_factory().db_ref()
    }

    /// Returns the configured `ProviderFactory`.
    pub const fn provider_factory(&self) -> &ProviderFactory<T::Types> {
        &self.right().db_provider_container.provider_factory
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    pub fn lookup_head(&self) -> eyre::Result<Head> {
        self.node_config()
            .lookup_head(self.provider_factory())
            .wrap_err("the head block is missing")
    }

    /// Returns the metrics sender.
    pub fn sync_metrics_tx(&self) -> UnboundedSender<MetricEvent> {
        self.right().db_provider_container.metrics_sender.clone()
    }

    /// Returns a reference to the blockchain provider.
    pub const fn blockchain_db(&self) -> &T::Provider {
        &self.right().blockchain_db
    }

    /// Returns a reference to the `BlockchainTreeConfig`.
    pub const fn tree_config(&self) -> &BlockchainTreeConfig {
        &self.right().tree_config
    }

    /// Returns the `CanonStateNotificationSender`.
    pub fn canon_state_notification_sender(&self) -> CanonStateNotificationSender {
        self.right().canon_state_notification_sender.clone()
    }

    /// Creates a `NodeAdapter` and attaches it to the launch context.
    pub async fn with_components<CB>(
        self,
        components_builder: CB,
        on_component_initialized: Box<
            dyn OnComponentInitializedHook<NodeAdapter<T, CB::Components>>,
        >,
    ) -> eyre::Result<
        LaunchContextWith<
            Attached<WithConfigs<<T::Types as NodeTypes>::ChainSpec>, WithComponents<T, CB>>,
        >,
    >
    where
        CB: NodeComponentsBuilder<T>,
    {
        // fetch the head block from the database
        let head = self.lookup_head()?;

        let builder_ctx = BuilderContext::new(
            head,
            self.blockchain_db().clone(),
            self.task_executor().clone(),
            self.configs().clone(),
        );

        debug!(target: "reth::cli", "creating components");
        let components = components_builder.build_components(&builder_ctx).await?;

        let consensus: Arc<dyn Consensus> = Arc::new(components.consensus().clone());

        let tree_externals = TreeExternals::new(
            self.provider_factory().clone().with_prune_modes(self.prune_modes()),
            consensus.clone(),
            components.block_executor().clone(),
        );
        let tree = BlockchainTree::new(tree_externals, *self.tree_config())?
            .with_sync_metrics_tx(self.sync_metrics_tx())
            // Note: This is required because we need to ensure that both the components and the
            // tree are using the same channel for canon state notifications. This will be removed
            // once the Blockchain provider no longer depends on an instance of the tree
            .with_canon_state_notification_sender(self.canon_state_notification_sender());

        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));

        // Replace the tree component with the actual tree
        let blockchain_db = self.blockchain_db().clone().set_tree(blockchain_tree);

        debug!(target: "reth::cli", "configured blockchain tree");

        let node_adapter = NodeAdapter {
            components,
            task_executor: self.task_executor().clone(),
            provider: blockchain_db.clone(),
        };

        debug!(target: "reth::cli", "calling on_component_initialized hook");
        on_component_initialized.on_event(node_adapter.clone())?;

        let components_container = WithComponents {
            db_provider_container: WithMeteredProvider {
                provider_factory: self.provider_factory().clone(),
                metrics_sender: self.sync_metrics_tx(),
            },
            blockchain_db,
            tree_config: self.right().tree_config,
            node_adapter,
            head,
            consensus,
        };

        let ctx = LaunchContextWith {
            inner: self.inner,
            attachment: self.attachment.map_right(|_| components_container),
        };

        Ok(ctx)
    }
}

impl<T, CB>
    LaunchContextWith<
        Attached<WithConfigs<<T::Types as NodeTypes>::ChainSpec>, WithComponents<T, CB>>,
    >
where
    T: FullNodeTypes<
        Provider: WithTree,
        Types: NodeTypes<ChainSpec: EthChainSpec + EthereumHardforks>,
    >,
    CB: NodeComponentsBuilder<T>,
{
    /// Returns the configured `ProviderFactory`.
    pub const fn provider_factory(&self) -> &ProviderFactory<T::Types> {
        &self.right().db_provider_container.provider_factory
    }

    /// Returns the max block that the node should run to, looking it up from the network if
    /// necessary
    pub async fn max_block<C>(&self, client: C) -> eyre::Result<Option<BlockNumber>>
    where
        C: HeadersClient,
    {
        self.node_config().max_block(client, self.provider_factory().clone()).await
    }

    /// Returns the static file provider to interact with the static files.
    pub fn static_file_provider(&self) -> StaticFileProvider {
        self.provider_factory().static_file_provider()
    }

    /// Creates a new [`StaticFileProducer`] with the attached database.
    pub fn static_file_producer(&self) -> StaticFileProducer<ProviderFactory<T::Types>> {
        StaticFileProducer::new(self.provider_factory().clone(), self.prune_modes())
    }

    /// Returns the current head block.
    pub const fn head(&self) -> Head {
        self.right().head
    }

    /// Returns the configured `NodeAdapter`.
    pub const fn node_adapter(&self) -> &NodeAdapter<T, CB::Components> {
        &self.right().node_adapter
    }

    /// Returns a reference to the blockchain provider.
    pub const fn blockchain_db(&self) -> &T::Provider {
        &self.right().blockchain_db
    }

    /// Returns the initial backfill to sync to at launch.
    ///
    /// This returns the configured `debug.tip` if set, otherwise it will check if backfill was
    /// previously interrupted and returns the block hash of the last checkpoint, see also
    /// [`Self::check_pipeline_consistency`]
    pub fn initial_backfill_target(&self) -> ProviderResult<Option<B256>> {
        let mut initial_target = self.node_config().debug.tip;

        if initial_target.is_none() {
            initial_target = self.check_pipeline_consistency()?;
        }

        Ok(initial_target)
    }

    /// Check if the pipeline is consistent (all stages have the checkpoint block numbers no less
    /// than the checkpoint of the first stage).
    ///
    /// This will return the pipeline target if:
    ///  * the pipeline was interrupted during its previous run
    ///  * a new stage was added
    ///  * stage data was dropped manually through `reth stage drop ...`
    ///
    /// # Returns
    ///
    /// A target block hash if the pipeline is inconsistent, otherwise `None`.
    pub fn check_pipeline_consistency(&self) -> ProviderResult<Option<B256>> {
        // If no target was provided, check if the stages are congruent - check if the
        // checkpoint of the last stage matches the checkpoint of the first.
        let first_stage_checkpoint = self
            .blockchain_db()
            .get_stage_checkpoint(*StageId::ALL.first().unwrap())?
            .unwrap_or_default()
            .block_number;

        // Skip the first stage as we've already retrieved it and comparing all other checkpoints
        // against it.
        for stage_id in StageId::ALL.iter().skip(1) {
            let stage_checkpoint = self
                .blockchain_db()
                .get_stage_checkpoint(*stage_id)?
                .unwrap_or_default()
                .block_number;

            // If the checkpoint of any stage is less than the checkpoint of the first stage,
            // retrieve and return the block hash of the latest header and use it as the target.
            if stage_checkpoint < first_stage_checkpoint {
                debug!(
                    target: "consensus::engine",
                    first_stage_checkpoint,
                    inconsistent_stage_id = %stage_id,
                    inconsistent_stage_checkpoint = stage_checkpoint,
                    "Pipeline sync progress is inconsistent"
                );
                return self.blockchain_db().block_hash(first_stage_checkpoint);
            }
        }

        Ok(None)
    }

    /// Returns the configured `Consensus`.
    pub fn consensus(&self) -> Arc<dyn Consensus> {
        self.right().consensus.clone()
    }

    /// Returns the metrics sender.
    pub fn sync_metrics_tx(&self) -> UnboundedSender<MetricEvent> {
        self.right().db_provider_container.metrics_sender.clone()
    }

    /// Returns a reference to the `BlockchainTreeConfig`.
    pub const fn tree_config(&self) -> &BlockchainTreeConfig {
        &self.right().tree_config
    }

    /// Returns the node adapter components.
    pub const fn components(&self) -> &CB::Components {
        &self.node_adapter().components
    }
}

impl<T, CB>
    LaunchContextWith<
        Attached<WithConfigs<<T::Types as NodeTypes>::ChainSpec>, WithComponents<T, CB>>,
    >
where
    T: FullNodeTypes<
        Provider: WithTree + StateProviderFactory + ChainSpecProvider,
        Types: NodeTypes<ChainSpec: EthereumHardforks>,
    >,
    CB: NodeComponentsBuilder<T>,
{
    /// Returns the [`InvalidBlockHook`] to use for the node.
    pub fn invalid_block_hook(&self) -> eyre::Result<Box<dyn InvalidBlockHook>> {
        let Some(ref hook) = self.node_config().debug.invalid_block_hook else {
            return Ok(Box::new(NoopInvalidBlockHook::default()))
        };
        let healthy_node_rpc_client = self.get_healthy_node_client()?;

        let output_directory = self.data_dir().invalid_block_hooks();
        let hooks = hook
            .iter()
            .copied()
            .map(|hook| {
                let output_directory = output_directory.join(hook.to_string());
                fs::create_dir_all(&output_directory)?;

                Ok(match hook {
                    InvalidBlockHookType::Witness => Box::new(InvalidBlockWitnessHook::new(
                        self.blockchain_db().clone(),
                        self.components().evm_config().clone(),
                        output_directory,
                        healthy_node_rpc_client.clone(),
                    )),
                    InvalidBlockHookType::PreState | InvalidBlockHookType::Opcode => {
                        eyre::bail!("invalid block hook {hook:?} is not implemented yet")
                    }
                } as Box<dyn InvalidBlockHook>)
            })
            .collect::<Result<_, _>>()?;

        Ok(Box::new(InvalidBlockHooks(hooks)))
    }

    /// Returns an RPC client for the healthy node, if configured in the node config.
    fn get_healthy_node_client(&self) -> eyre::Result<Option<jsonrpsee::http_client::HttpClient>> {
        self.node_config()
            .debug
            .healthy_node_rpc_url
            .as_ref()
            .map(|url| {
                let client = jsonrpsee::http_client::HttpClientBuilder::default().build(url)?;

                // Verify that the healthy node is running the same chain as the current node.
                let chain_id = futures::executor::block_on(async {
                    EthApiClient::<
                        alloy_rpc_types::Transaction,
                        alloy_rpc_types::Block,
                        alloy_rpc_types::Receipt,
                    >::chain_id(&client)
                    .await
                })?
                .ok_or_eyre("healthy node rpc client didn't return a chain id")?;
                if chain_id.to::<u64>() != self.chain_id().id() {
                    eyre::bail!("invalid chain id for healthy node: {chain_id}")
                }

                Ok(client)
            })
            .transpose()
    }
}

/// Joins two attachments together.
#[derive(Clone, Copy, Debug)]
pub struct Attached<L, R> {
    left: L,
    right: R,
}

impl<L, R> Attached<L, R> {
    /// Creates a new `Attached` with the given values.
    pub const fn new(left: L, right: R) -> Self {
        Self { left, right }
    }

    /// Maps the left value to a new value.
    pub fn map_left<F, T>(self, f: F) -> Attached<T, R>
    where
        F: FnOnce(L) -> T,
    {
        Attached::new(f(self.left), self.right)
    }

    /// Maps the right value to a new value.
    pub fn map_right<F, T>(self, f: F) -> Attached<L, T>
    where
        F: FnOnce(R) -> T,
    {
        Attached::new(self.left, f(self.right))
    }

    /// Get a reference to the left value.
    pub const fn left(&self) -> &L {
        &self.left
    }

    /// Get a reference to the right value.
    pub const fn right(&self) -> &R {
        &self.right
    }

    /// Get a mutable reference to the right value.
    pub fn left_mut(&mut self) -> &mut R {
        &mut self.right
    }

    /// Get a mutable reference to the right value.
    pub fn right_mut(&mut self) -> &mut R {
        &mut self.right
    }
}

/// Helper container type to bundle the initial [`NodeConfig`] and the loaded settings from the
/// reth.toml config
#[derive(Debug, Clone)]
pub struct WithConfigs<ChainSpec> {
    /// The configured, usually derived from the CLI.
    pub config: NodeConfig<ChainSpec>,
    /// The loaded reth.toml config.
    pub toml_config: reth_config::Config,
}

/// Helper container type to bundle the [`ProviderFactory`] and the metrics
/// sender.
#[derive(Debug, Clone)]
pub struct WithMeteredProvider<N: NodeTypesWithDB> {
    provider_factory: ProviderFactory<N>,
    metrics_sender: UnboundedSender<MetricEvent>,
}

/// Helper container to bundle the [`ProviderFactory`], [`BlockchainProvider`]
/// and a metrics sender.
#[allow(missing_debug_implementations)]
pub struct WithMeteredProviders<T>
where
    T: FullNodeTypes,
{
    db_provider_container: WithMeteredProvider<T::Types>,
    blockchain_db: T::Provider,
    canon_state_notification_sender: CanonStateNotificationSender,
    tree_config: BlockchainTreeConfig,
}

/// Helper container to bundle the metered providers container and [`NodeAdapter`].
#[allow(missing_debug_implementations)]
pub struct WithComponents<T, CB>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
{
    db_provider_container: WithMeteredProvider<T::Types>,
    tree_config: BlockchainTreeConfig,
    blockchain_db: T::Provider,
    node_adapter: NodeAdapter<T, CB::Components>,
    head: Head,
    consensus: Arc<dyn Consensus>,
}

#[cfg(test)]
mod tests {
    use super::{LaunchContext, NodeConfig};
    use reth_config::Config;
    use reth_node_core::args::PruningArgs;

    const EXTENSION: &str = "toml";

    fn with_tempdir(filename: &str, proc: fn(&std::path::Path)) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join(filename).with_extension(EXTENSION);
        proc(&config_path);
        temp_dir.close().unwrap()
    }

    #[test]
    fn test_save_prune_config() {
        with_tempdir("prune-store-test", |config_path| {
            let mut reth_config = Config::default();
            let node_config = NodeConfig {
                pruning: PruningArgs {
                    full: true,
                    block_interval: 0,
                    sender_recovery_full: false,
                    sender_recovery_distance: None,
                    sender_recovery_before: None,
                    transaction_lookup_full: false,
                    transaction_lookup_distance: None,
                    transaction_lookup_before: None,
                    receipts_full: false,
                    receipts_distance: None,
                    receipts_before: None,
                    account_history_full: false,
                    account_history_distance: None,
                    account_history_before: None,
                    storage_history_full: false,
                    storage_history_distance: None,
                    storage_history_before: None,
                    receipts_log_filter: vec![],
                },
                ..NodeConfig::test()
            };
            LaunchContext::save_pruning_config_if_full_node(
                &mut reth_config,
                &node_config,
                config_path,
            )
            .unwrap();

            let loaded_config = Config::from_path(config_path).unwrap();

            assert_eq!(reth_config, loaded_config);
        })
    }
}
