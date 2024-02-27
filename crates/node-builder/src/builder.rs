//! Customizable node builder.

#![allow(clippy::type_complexity, missing_debug_implementations)]

use crate::{
    components::{
        FullNodeComponents, FullNodeComponentsAdapter, NodeComponents, NodeComponentsBuilder,
    },
    hooks::NodeHooks,
    node::{FullNode, FullNodeTypes, FullNodeTypesAdapter, NodeTypes},
    rpc::{RethRpcServerHandles, RpcContext, RpcHooks},
    NodeHandle,
};
use eyre::Context;
use futures::{future::Either, stream, stream_select, StreamExt};
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensusEngine,
};
use reth_blockchain_tree::{BlockchainTreeConfig, ShareableBlockchainTree};
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_interfaces::p2p::either::EitherDownloader;
use reth_network::{
    transactions::{TransactionFetcherConfig, TransactionsManagerConfig},
    NetworkBuilder, NetworkEvents, NetworkHandle,
};
use reth_node_core::{
    cli::config::{PayloadBuilderConfig, RethRpcConfig, RethTransactionPoolConfig},
    dirs::{ChainPath, DataDirPath},
    events::cl::ConsensusLayerHealthEvents,
    exit::NodeExitFuture,
    init::init_genesis,
    node_config::NodeConfig,
    primitives::{kzg::KzgSettings, Head},
    utils::write_peers_to_file,
};
use reth_primitives::{
    constants::eip4844::{LoadKzgSettingsError, MAINNET_KZG_TRUSTED_SETUP},
    format_ether, ChainSpec,
};
use reth_provider::{providers::BlockchainProvider, ChainSpecProvider, ProviderFactory};
use reth_prune::{PrunerBuilder, PrunerEvent};
use reth_revm::EvmProcessorFactory;
use reth_rpc_engine_api::EngineApi;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{PoolConfig, TransactionPool};
use std::sync::Arc;
use tokio::sync::{mpsc::unbounded_channel, oneshot};

/// The builtin provider type of the reth node.
// Note: we need to hardcode this because custom components might depend on it in associated types.
type RethFullProviderType<DB, Evm> =
    BlockchainProvider<DB, ShareableBlockchainTree<DB, EvmProcessorFactory<Evm>>>;

/// Declaratively construct a node.
///
/// [`NodeBuilder`] provides a [builder-like interface][builder] for composing
/// components of a node.
///
/// Configuring a node starts out with a [`NodeConfig`] and then proceeds to configure the core
/// static types of the node: [NodeTypes], these include the node's primitive types and the node's
/// engine types.
///
/// Next all stateful components of the node are configured, these include the
/// [ConfigureEvm](reth_node_api::evm::ConfigureEvm), the database [Database] and finally all the
/// components of the node that are downstream of those types, these include:
///
///  - The transaction pool: [PoolBuilder](crate::components::PoolBuilder)
///  - The network: [NetworkBuilder](crate::components::NetworkBuilder)
///  - The payload builder: [PayloadBuilder](crate::components::PayloadServiceBuilder)
///
/// Finally, the node is ready to launch [NodeBuilder::launch]
///
/// [builder]: https://doc.rust-lang.org/1.0.0/style/ownership/builders.html
pub struct NodeBuilder<DB, State> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// State of the node builder process.
    state: State,
    /// The configured database for the node.
    database: DB,
}

impl<DB, State> NodeBuilder<DB, State> {
    /// Returns a reference to the node builder's config.
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self, data_dir: &ChainPath<DataDirPath>) -> eyre::Result<reth_config::Config> {
        let config_path = self.config.config.clone().unwrap_or_else(|| data_dir.config_path());

        let mut config = confy::load_path::<reth_config::Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.config.network.trusted_only;

        if !self.config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.config.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
    }
}

impl NodeBuilder<(), InitState> {
    /// Create a new [`NodeBuilder`].
    pub fn new(config: NodeConfig) -> Self {
        Self { config, database: (), state: InitState::default() }
    }
}

impl<DB> NodeBuilder<DB, InitState> {
    /// Configures the additional external context, e.g. additional context captured via CLI args.
    pub fn with_database<D>(self, database: D) -> NodeBuilder<D, InitState> {
        NodeBuilder { config: self.config, state: self.state, database }
    }
}

impl<DB> NodeBuilder<DB, InitState>
where
    DB: Database + Clone + 'static,
{
    /// Configures the types of the node.
    pub fn with_types<T>(self, types: T) -> NodeBuilder<DB, TypesState<T, DB>>
    where
        T: NodeTypes,
    {
        NodeBuilder {
            config: self.config,
            state: TypesState { adapter: FullNodeTypesAdapter::new(types) },
            database: self.database,
        }
    }
}

impl<DB, Types> NodeBuilder<DB, TypesState<Types, DB>>
where
    Types: NodeTypes,
    DB: Database + Clone + Unpin + 'static,
{
    /// Configures the node's components.
    pub fn with_components<Components>(
        self,
        components_builder: Components,
    ) -> NodeBuilder<
        DB,
        ComponentsState<
            Types,
            Components,
            FullNodeComponentsAdapter<
                FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                Components::Pool,
            >,
        >,
    >
    where
        Components: NodeComponentsBuilder<
            FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
        >,
    {
        NodeBuilder {
            config: self.config,
            database: self.database,
            state: ComponentsState {
                types: self.state.adapter.types,
                components_builder,
                hooks: NodeHooks::new(),
                rpc: RpcHooks::new(),
            },
        }
    }
}

impl<DB, Types, Components>
    NodeBuilder<
        DB,
        ComponentsState<
            Types,
            Components,
            FullNodeComponentsAdapter<
                FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                Components::Pool,
            >,
        >,
    >
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
    Types: NodeTypes,
    Components: NodeComponentsBuilder<
        FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
    >,
{
    /// Apply a function to the components builder.
    pub fn map_components(self, f: impl FnOnce(Components) -> Components) -> Self {
        Self {
            config: self.config,
            database: self.database,
            state: ComponentsState {
                types: self.state.types,
                components_builder: f(self.state.components_builder),
                hooks: self.state.hooks,
                rpc: self.state.rpc,
            },
        }
    }

    /// Resets the setup process to the components stage.
    ///
    /// CAUTION: All previously configured hooks will be lost.
    pub fn fuse_components<C>(
        self,
        components_builder: C,
    ) -> NodeBuilder<
        DB,
        ComponentsState<
            Types,
            C,
            FullNodeComponentsAdapter<
                FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                C::Pool,
            >,
        >,
    >
    where
        C: NodeComponentsBuilder<
            FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
        >,
    {
        NodeBuilder {
            config: self.config,
            database: self.database,
            state: ComponentsState {
                types: self.state.types,
                components_builder,
                hooks: NodeHooks::new(),
                rpc: RpcHooks::new(),
            },
        }
    }

    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: Fn(
                FullNodeComponentsAdapter<
                    FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                    Components::Pool,
                >,
            ) -> eyre::Result<()>
            + 'static,
    {
        self.state.hooks.set_on_component_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: Fn(
                FullNode<
                    FullNodeComponentsAdapter<
                        FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                        Components::Pool,
                    >,
                >,
            ) -> eyre::Result<()>
            + 'static,
    {
        self.state.hooks.set_on_node_started(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: Fn(
                RpcContext<
                    '_,
                    FullNodeComponentsAdapter<
                        FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                        Components::Pool,
                    >,
                >,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + 'static,
    {
        self.state.rpc.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: Fn(
                RpcContext<
                    '_,
                    FullNodeComponentsAdapter<
                        FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                        Components::Pool,
                    >,
                >,
            ) -> eyre::Result<()>
            + 'static,
    {
        self.state.rpc.set_extend_rpc_modules(hook);
        self
    }

    /// Launches the node and returns a handle to it.
    pub async fn launch(
        self,
        executor: TaskExecutor,
        data_dir: ChainPath<DataDirPath>,
    ) -> eyre::Result<
        NodeHandle<
            FullNodeComponentsAdapter<
                FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                Components::Pool,
            >,
        >,
    > {
        // get config
        let reth_config = self.load_config(&data_dir)?;

        let Self {
            config,
            state: ComponentsState { types, components_builder, hooks, rpc },
            database,
        } = self;

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit()?;

        let prometheus_handle = config.install_prometheus_recorder()?;
        config.start_metrics_endpoint(prometheus_handle, database.clone()).await?;

        info!(target: "reth::cli", "Database opened");

        let mut provider_factory =
            ProviderFactory::new(database.clone(), Arc::clone(&config.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            data_dir.snapshots_path(),
            config.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory
            .with_snapshots(data_dir.snapshots_path(), snapshotter.highest_snapshot_receiver())?;

        debug!(target: "reth::cli", chain=%config.chain.chain, genesis=?config.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(database.clone(), config.chain.clone())?;

        info!(target: "reth::cli", "{}", config.chain.display_hardforks());

        let consensus = config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config = config.prune_config()?.or(reth_config.prune.clone());

        let evm_config = types.evm_config();
        let tree_config = BlockchainTreeConfig::default();
        let tree = config.build_blockchain_tree(
            provider_factory.clone(),
            consensus.clone(),
            prune_config.clone(),
            sync_metrics_tx.clone(),
            tree_config,
            evm_config.clone(),
        )?;

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head =
            config.lookup_head(provider_factory.clone()).wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        let ctx = BuilderContext {
            head,
            provider: blockchain_db,
            executor,
            data_dir,
            config,
            reth_config,
        };

        debug!(target: "reth::cli", "creating components");
        let NodeComponents { transaction_pool, network, payload_builder } =
            components_builder.build_components(&ctx)?;

        let BuilderContext {
            provider: blockchain_db,
            executor,
            data_dir,
            mut config,
            reth_config,
            ..
        } = ctx;

        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        let node_components = FullNodeComponentsAdapter {
            evm_config: evm_config.clone(),
            pool: transaction_pool.clone(),
            network: network.clone(),
            provider: blockchain_db.clone(),
            payload_builder: payload_builder.clone(),
            executor: executor.clone(),
        };
        debug!(target: "reth::cli", "calling on_component_initialized hook");
        on_component_initialized.on_event(node_components.clone())?;

        // create pipeline
        let network_client = network.fetch_client().await?;
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
        let max_block = config.max_block(&network_client, provider_factory.clone()).await?;

        // Configure the pipeline
        let (mut pipeline, client) = if config.dev.dev {
            info!(target: "reth::cli", "Starting Reth in dev mode");
            for (idx, (address, alloc)) in config.chain.genesis.alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx, address.to_string(), format_ether(alloc.balance));
            }

            let mining_mode = config.mining_mode(transaction_pool.pending_transactions_listener());

            let (_, client, mut task) = reth_auto_seal_consensus::AutoSealBuilder::new(
                Arc::clone(&config.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
                evm_config.clone(),
            )
            .build();

            let mut pipeline = config
                .build_networked_pipeline(
                    &reth_config.stages,
                    client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                    evm_config,
                )
                .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            executor.spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = config
                .build_networked_pipeline(
                    &reth_config.stages,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                    evm_config,
                )
                .await?;

            (pipeline, EitherDownloader::Right(network_client))
        };

        let pipeline_events = pipeline.events();

        let initial_target = config.initial_pipeline_target(genesis_hash);
        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = PrunerBuilder::new(prune_config.clone())
                .max_reorg_depth(tree_config.max_reorg_depth() as usize)
                .prune_delete_limit(config.chain.prune_delete_limit)
                .build(provider_factory, snapshotter.highest_snapshot_receiver());

            let events = pruner.events();
            hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));

            info!(target: "reth::cli", ?prune_config, "Pruner initialized");
            Either::Left(events)
        } else {
            Either::Right(stream::empty::<PrunerEvent>())
        };

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(executor.clone()),
            Box::new(network.clone()),
            max_block,
            config.debug.continuous,
            payload_builder.clone(),
            initial_target,
            reth_beacon_consensus::MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if config.debug.tip.is_none() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into)
        );
        executor.spawn_critical(
            "events task",
            reth_node_core::events::node::handle_events(
                Some(network.clone()),
                Some(head.number),
                events,
                database.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            config.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = data_dir.jwt_path();
        let jwt_secret = config.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        config.adjust_instance_ports();

        // Start RPC servers

        let (rpc_server_handles, rpc_registry) = crate::rpc::launch_rpc_servers(
            node_components.clone(),
            engine_api,
            &config,
            jwt_secret,
            rpc,
        )
        .await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let FullNodeComponentsAdapter {
            evm_config,
            pool,
            network,
            provider,
            payload_builder,
            executor,
        } = node_components;

        let full_node = FullNode {
            evm_config,
            pool,
            network,
            provider,
            payload_builder,
            executor,
            rpc_server_handles,
            rpc_registry,
            config,
            data_dir,
        };
        // Notify on node started
        on_node_started.on_event(full_node.clone())?;

        let handle = NodeHandle {
            node_exit_future: NodeExitFuture::new(rx, full_node.config.debug.terminate),
            node: full_node,
        };

        Ok(handle)
    }

    /// Check that the builder can be launched
    ///
    /// This is useful when writing tests to ensure that the builder is configured correctly.
    pub fn check_launch(self) -> Self {
        self
    }
}

/// Captures the necessary context for building the components of the node.
#[derive(Debug)]
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    head: Head,
    /// The configured provider to interact with the blockchain.
    provider: Node::Provider,
    /// The executor of the node.
    executor: TaskExecutor,
    /// The data dir of the node.
    data_dir: ChainPath<DataDirPath>,
    /// The config of the node
    config: NodeConfig,
    /// loaded config
    reth_config: reth_config::Config,
}

impl<Node: FullNodeTypes> BuilderContext<Node> {
    /// Returns the configured provider to interact with the blockchain.
    pub fn provider(&self) -> &Node::Provider {
        &self.provider
    }

    /// Returns the current head of the blockchain at launch.
    pub fn head(&self) -> Head {
        self.head
    }

    /// Returns the config of the node.
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Returns the data dir of the node.
    ///
    /// This gives access to all relevant files and directories of the node's datadir.
    pub fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.data_dir
    }

    /// Returns the executor of the node.
    ///
    /// This can be used to execute async tasks or functions during the setup.
    pub fn task_executor(&self) -> &TaskExecutor {
        &self.executor
    }

    /// Returns the chain spec of the node.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.provider().chain_spec()
    }

    /// Returns the transaction pool config of the node.
    pub fn pool_config(&self) -> PoolConfig {
        self.config().txpool.pool_config()
    }

    /// Loads the trusted setup params from a given file path or falls back to
    /// `MAINNET_KZG_TRUSTED_SETUP`.
    pub fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        if let Some(ref trusted_setup_file) = self.config().trusted_setup_file {
            let trusted_setup = KzgSettings::load_trusted_setup_file(trusted_setup_file)
                .map_err(LoadKzgSettingsError::KzgError)?;
            Ok(Arc::new(trusted_setup))
        } else {
            Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
        }
    }

    /// Returns the config for payload building.
    pub fn payload_builder_config(&self) -> impl PayloadBuilderConfig {
        self.config.builder.clone()
    }

    /// Creates the [NetworkBuilder] for the node.
    pub async fn network_builder(&self) -> eyre::Result<NetworkBuilder<Node::Provider, (), ()>> {
        self.config
            .build_network(
                &self.reth_config,
                self.provider.clone(),
                self.executor.clone(),
                self.head,
                self.data_dir(),
            )
            .await
    }

    /// Creates the [NetworkBuilder] for the node and blocks until it is ready.
    pub fn network_builder_blocking(&self) -> eyre::Result<NetworkBuilder<Node::Provider, (), ()>> {
        self.executor.block_on(self.network_builder())
    }

    /// Spawns the configured network and associated tasks and returns the [NetworkHandle] connected
    /// to that network.
    pub fn start_network<Pool>(
        &self,
        builder: NetworkBuilder<Node::Provider, (), ()>,
        pool: Pool,
    ) -> NetworkHandle
    where
        Pool: TransactionPool + Unpin + 'static,
    {
        let (handle, network, txpool, eth) = builder
            .transactions(
                pool,
                TransactionsManagerConfig {
                    transaction_fetcher_config: TransactionFetcherConfig::new(
                        self.config.network.soft_limit_byte_size_pooled_transactions_response,
                        self.config
                            .network
                            .soft_limit_byte_size_pooled_transactions_response_on_pack_request,
                    ),
                },
            )
            .request_handler(self.provider().clone())
            .split_with_handle();

        self.executor.spawn_critical("p2p txpool", txpool);
        self.executor.spawn_critical("p2p eth request handler", eth);

        let default_peers_path = self.data_dir().known_peers_path();
        let known_peers_file = self.config.network.persistent_peers_file(default_peers_path);
        self.executor.spawn_critical_with_graceful_shutdown_signal(
            "p2p network task",
            |shutdown| {
                network.run_until_graceful_shutdown(shutdown, |network| {
                    write_peers_to_file(network, known_peers_file)
                })
            },
        );

        handle
    }
}

/// The initial state of the node builder process.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct InitState;

/// The state after all types of the node have been configured.
#[derive(Debug)]
pub struct TypesState<Types, DB>
where
    DB: Database + Clone + 'static,
    Types: NodeTypes,
{
    adapter: FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
}

/// The state of the node builder process after the node's components have been configured.
///
/// With this state all types and components of the node are known and the node can be launched.
///
/// Additionally, this state captures additional hooks that are called at specific points in the
/// node's launch lifecycle.
#[derive(Debug)]
pub struct ComponentsState<Types, Components, FullNode: FullNodeComponents> {
    /// The types of the node.
    types: Types,
    /// Type that builds the components of the node.
    components_builder: Components,
    /// Additional NodeHooks that are called at specific points in the node's launch lifecycle.
    hooks: NodeHooks<FullNode>,
    /// Additional RPC hooks.
    rpc: RpcHooks<FullNode>,
}
