//! Customizable node builder.

#![allow(clippy::type_complexity, missing_debug_implementations)]

use crate::{
    components::{
        ComponentsBuilder, FullNodeComponents, FullNodeComponentsAdapter, NodeComponents,
        NodeComponentsBuilder, PoolBuilder,
    },
    hooks::NodeHooks,
    node::{FullNode, FullNodeTypes, FullNodeTypesAdapter},
    rpc::{RethRpcServerHandles, RpcContext, RpcHooks},
    Node, NodeHandle,
};
use eyre::Context;
use futures::{future::Either, stream, stream_select, StreamExt};
use rayon::ThreadPoolBuilder;
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook, StaticFileHook},
    BeaconConsensusEngine,
};
use reth_blockchain_tree::{BlockchainTreeConfig, ShareableBlockchainTree};
use reth_config::config::EtlConfig;
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_interfaces::p2p::either::EitherDownloader;
use reth_network::{NetworkBuilder, NetworkConfig, NetworkEvents, NetworkHandle};
use reth_node_api::NodeTypes;
use reth_node_core::{
    cli::config::{PayloadBuilderConfig, RethRpcConfig, RethTransactionPoolConfig},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    engine_api_store::EngineApiStore,
    events::cl::ConsensusLayerHealthEvents,
    exit::NodeExitFuture,
    init::init_genesis,
    node_config::NodeConfig,
    primitives::{kzg::KzgSettings, Head},
    utils::write_peers_to_file,
};
use reth_primitives::{constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, format_ether, ChainSpec};
use reth_provider::{providers::BlockchainProvider, ChainSpecProvider, ProviderFactory};
use reth_prune::PrunerBuilder;
use reth_revm::EvmProcessorFactory;
use reth_rpc_engine_api::EngineApi;
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, error, info};
use reth_transaction_pool::{PoolConfig, TransactionPool};
use std::{cmp::max, str::FromStr, sync::Arc, thread::available_parallelism};
use tokio::sync::{mpsc::unbounded_channel, oneshot};

/// The builtin provider type of the reth node.
// Note: we need to hardcode this because custom components might depend on it in associated types.
type RethFullProviderType<DB, Evm> =
    BlockchainProvider<DB, ShareableBlockchainTree<DB, EvmProcessorFactory<Evm>>>;

type RethFullAdapter<DB, N> =
    FullNodeTypesAdapter<N, DB, RethFullProviderType<DB, <N as NodeTypes>::Evm>>;

/// Declaratively construct a node.
///
/// [`NodeBuilder`] provides a [builder-like interface][builder] for composing
/// components of a node.
///
/// ## Order
///
/// Configuring a node starts out with a [`NodeConfig`] (this can be obtained from cli arguments for
/// example) and then proceeds to configure the core static types of the node: [NodeTypes], these
/// include the node's primitive types and the node's engine types.
///
/// Next all stateful components of the node are configured, these include the
/// [ConfigureEvm](reth_node_api::evm::ConfigureEvm), the database [Database] and all the
/// components of the node that are downstream of those types, these include:
///
///  - The transaction pool: [PoolBuilder]
///  - The network: [NetworkBuilder](crate::components::NetworkBuilder)
///  - The payload builder: [PayloadBuilder](crate::components::PayloadServiceBuilder)
///
/// Once all the components are configured, the node is ready to be launched.
///
/// On launch the builder returns a fully type aware [NodeHandle] that has access to all the
/// configured components and can interact with the node.
///
/// There are convenience functions for networks that come with a preset of types and components via
/// the [Node] trait, see `reth_node_ethereum::EthereumNode` or `reth_node_optimism::OptimismNode`.
///
/// The [NodeBuilder::node] function configures the node's types and components in one step.
///
/// ## Components
///
/// All components are configured with a [NodeComponentsBuilder] that is responsible for actually
/// creating the node components during the launch process. The [ComponentsBuilder] is a general
/// purpose implementation of the [NodeComponentsBuilder] trait that can be used to configure the
/// network, transaction pool and payload builder of the node. It enforces the correct order of
/// configuration, for example the network and the payload builder depend on the transaction pool
/// type that is configured first.
///
/// All builder traits are generic over the node types and are invoked with the [BuilderContext]
/// that gives access to internals of the that are needed to configure the components. This include
/// the original config, chain spec, the database provider and the task executor,
///
/// ## Hooks
///
/// Once all the components are configured, the builder can be used to set hooks that are run at
/// specific points in the node's lifecycle. This way custom services can be spawned before the node
/// is launched [NodeBuilder::on_component_initialized], or once the rpc server(s) are launched
/// [NodeBuilder::on_rpc_started]. The [NodeBuilder::extend_rpc_modules] can be used to inject
/// custom rpc modules into the rpc server before it is launched. See also [RpcContext]
/// All hooks accept a closure that is then invoked at the appropriate time in the node's launch
/// process.
///
/// ## Internals
///
/// The node builder is fully type safe, it uses the [NodeTypes] trait to enforce that all
/// components are configured with the correct types. However the database types and with that the
/// provider trait implementations are currently created by the builder itself during the launch
/// process, hence the database type is not part of the [NodeTypes] trait and the node's components,
/// that depend on the database, are configured separately. In order to have a nice trait that
/// encapsulates the entire node the [FullNodeComponents] trait was introduced. This trait has
/// convenient associated types for all the components of the node. After [NodeBuilder::launch] the
/// [NodeHandle] contains an instance of [FullNode] that implements the [FullNodeComponents] trait
/// and has access to all the components of the node. Internally the node builder uses several
/// generic adapter types that are then map to traits with associated types for ease of use.
///
/// ### Limitations
///
/// Currently the launch process is limited to ethereum nodes and requires all the components
/// specified above. It also expect beacon consensus with the ethereum engine API that is configured
/// by the builder itself during launch. This might change in the future.
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
            .wrap_err_with(|| format!("Could not load config file {config_path:?}"))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.trusted_nodes_only = self.config.network.trusted_only;

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
    /// Configures the underlying database that the node will use.
    pub fn with_database<D>(self, database: D) -> NodeBuilder<D, InitState> {
        NodeBuilder { config: self.config, state: self.state, database }
    }

    /// Preconfigure the builder with the context to launch the node.
    ///
    /// This provides the task executor and the data directory for the node.
    pub fn with_launch_context(
        self,
        task_executor: TaskExecutor,
        data_dir: ChainPath<DataDirPath>,
    ) -> WithLaunchContext<DB, InitState> {
        WithLaunchContext { builder: self, task_executor, data_dir }
    }

    /// Creates an _ephemeral_ preconfigured node for testing purposes.
    pub fn testing_node(
        self,
        task_executor: TaskExecutor,
    ) -> WithLaunchContext<Arc<TempDatabase<DatabaseEnv>>, InitState> {
        let db = create_test_rw_db();
        let db_path_str = db.path().to_str().expect("Path is not valid unicode");
        let path =
            MaybePlatformPath::<DataDirPath>::from_str(db_path_str).expect("Path is not valid");
        let data_dir = path.unwrap_or_chain_default(self.config.chain.chain);

        WithLaunchContext { builder: self.with_database(db), task_executor, data_dir }
    }
}

impl<DB> NodeBuilder<DB, InitState>
where
    DB: Database + Unpin + Clone + 'static,
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

    /// Preconfigures the node with a specific node implementation.
    ///
    /// This is a convenience method that sets the node's types and components in one call.
    pub fn node<N>(
        self,
        node: N,
    ) -> NodeBuilder<
        DB,
        ComponentsState<
            N,
            ComponentsBuilder<
                RethFullAdapter<DB, N>,
                N::PoolBuilder,
                N::PayloadBuilder,
                N::NetworkBuilder,
            >,
            FullNodeComponentsAdapter<
                RethFullAdapter<DB, N>,
                <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
            >,
        >,
    >
    where
        N: Node<FullNodeTypesAdapter<N, DB, RethFullProviderType<DB, <N as NodeTypes>::Evm>>>,
        N::PoolBuilder: PoolBuilder<RethFullAdapter<DB, N>>,
        N::NetworkBuilder: crate::components::NetworkBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
        N::PayloadBuilder: crate::components::PayloadServiceBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
    {
        self.with_types(node.clone()).with_components(node.components())
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

    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: Fn(
                FullNodeComponentsAdapter<
                    FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                    Components::Pool,
                >,
            ) -> eyre::Result<()>
            + Send
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
            + Send
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
            + Send
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
            + Send
            + 'static,
    {
        self.state.rpc.set_extend_rpc_modules(hook);
        self
    }

    /// Launches the node and returns a handle to it.
    ///
    /// This bootstraps the node internals, creates all the components with the provider
    /// [NodeComponentsBuilder] and launches the node.
    ///
    /// Returns a [NodeHandle] that can be used to interact with the node.
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
        // get config from file
        let reth_config = self.load_config(&data_dir)?;

        let Self {
            config,
            state: ComponentsState { types, components_builder, hooks, rpc },
            database,
        } = self;

        // Raise the fd limit of the process.
        // Does not do anything on windows.
        fdlimit::raise_fd_limit()?;

        // Limit the global rayon thread pool, reserving 2 cores for the rest of the system
        let _ = ThreadPoolBuilder::new()
            .num_threads(
                available_parallelism().map_or(25, |cpus| max(cpus.get().saturating_sub(2), 2)),
            )
            .build_global()
            .map_err(|e| error!("Failed to build global thread pool: {:?}", e));

        let provider_factory = ProviderFactory::new(
            database.clone(),
            Arc::clone(&config.chain),
            data_dir.static_files_path(),
        )?
        .with_static_files_metrics();
        info!(target: "reth::cli", "Database opened");

        let prometheus_handle = config.install_prometheus_recorder()?;
        config
            .start_metrics_endpoint(
                prometheus_handle,
                database.clone(),
                provider_factory.static_file_provider(),
            )
            .await?;

        debug!(target: "reth::cli", chain=%config.chain.chain, genesis=?config.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(provider_factory.clone())?;

        info!(target: "reth::cli", "\n{}", config.chain.display_hardforks());

        let consensus = config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config = config.prune_config()?.or_else(|| reth_config.prune.clone());

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
            components_builder.build_components(&ctx).await?;

        let BuilderContext {
            provider: blockchain_db,
            executor,
            data_dir,
            mut config,
            mut reth_config,
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
        let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();

        if let Some(store_path) = config.debug.engine_api_store.clone() {
            debug!(target: "reth::cli", "spawning engine API store");
            let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
            let engine_api_store = EngineApiStore::new(store_path);
            executor.spawn_critical(
                "engine api interceptor",
                engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
            );
            consensus_engine_rx = engine_intercept_rx;
        };

        let max_block = config.max_block(&network_client, provider_factory.clone()).await?;
        let mut hooks = EngineHooks::new();

        let static_file_producer = StaticFileProducer::new(
            provider_factory.clone(),
            provider_factory.static_file_provider(),
            prune_config.clone().unwrap_or_default().segments,
        );
        let static_file_producer_events = static_file_producer.lock().events();
        hooks.add(StaticFileHook::new(static_file_producer.clone(), Box::new(executor.clone())));
        info!(target: "reth::cli", "StaticFileProducer initialized");

        // Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
        if reth_config.stages.etl.dir.is_none() {
            reth_config.stages.etl.dir = Some(EtlConfig::from_datadir(&data_dir.data_dir_path()));
        }

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
                    static_file_producer,
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
                    static_file_producer,
                    evm_config,
                )
                .await?;

            (pipeline, EitherDownloader::Right(network_client))
        };

        let pipeline_events = pipeline.events();

        let initial_target = config.initial_pipeline_target(genesis_hash);

        let prune_config = prune_config.unwrap_or_default();
        let mut pruner = PrunerBuilder::new(prune_config.clone())
            .max_reorg_depth(tree_config.max_reorg_depth() as usize)
            .prune_delete_limit(config.chain.prune_delete_limit)
            .build(provider_factory.clone());

        let pruner_events = pruner.events();
        hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));
        info!(target: "reth::cli", ?prune_config, "Pruner initialized");

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
            if config.debug.tip.is_none() && !config.dev.dev {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into),
            static_file_producer_events.map(Into::into)
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

        let (rpc_server_handles, mut rpc_registry) = crate::rpc::launch_rpc_servers(
            node_components.clone(),
            engine_api,
            &config,
            jwt_secret,
            rpc,
        )
        .await?;

        // in dev mode we generate 20 random dev-signer accounts
        if config.dev.dev {
            rpc_registry.eth_api().with_dev_accounts();
        }

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
            task_executor: executor,
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

/// A [NodeBuilder] with it's launch context already configured.
///
/// This exposes the same methods as [NodeBuilder] but with the launch context already configured,
/// See [WithLaunchContext::launch]
pub struct WithLaunchContext<DB, State> {
    builder: NodeBuilder<DB, State>,
    task_executor: TaskExecutor,
    data_dir: ChainPath<DataDirPath>,
}

impl<DB, State> WithLaunchContext<DB, State> {
    /// Returns a reference to the node builder's config.
    pub fn config(&self) -> &NodeConfig {
        self.builder.config()
    }

    /// Returns a reference to the task executor.
    pub fn task_executor(&self) -> &TaskExecutor {
        &self.task_executor
    }

    /// Returns a reference to the data directory.
    pub fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.data_dir
    }
}

impl<DB> WithLaunchContext<DB, InitState>
where
    DB: Database + Clone + Unpin + 'static,
{
    /// Configures the types of the node.
    pub fn with_types<T>(self, types: T) -> WithLaunchContext<DB, TypesState<T, DB>>
    where
        T: NodeTypes,
    {
        WithLaunchContext {
            builder: self.builder.with_types(types),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Preconfigures the node with a specific node implementation.
    pub fn node<N>(
        self,
        node: N,
    ) -> WithLaunchContext<
        DB,
        ComponentsState<
            N,
            ComponentsBuilder<
                RethFullAdapter<DB, N>,
                N::PoolBuilder,
                N::PayloadBuilder,
                N::NetworkBuilder,
            >,
            FullNodeComponentsAdapter<
                RethFullAdapter<DB, N>,
                <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
            >,
        >,
    >
    where
        N: Node<FullNodeTypesAdapter<N, DB, RethFullProviderType<DB, <N as NodeTypes>::Evm>>>,
        N::PoolBuilder: PoolBuilder<RethFullAdapter<DB, N>>,
        N::NetworkBuilder: crate::components::NetworkBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
        N::PayloadBuilder: crate::components::PayloadServiceBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
    {
        self.with_types(node.clone()).with_components(node.components())
    }
}

impl<DB> WithLaunchContext<DB, InitState>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    /// Launches a preconfigured [Node]
    ///
    /// This bootstraps the node internals, creates all the components with the given [Node] type
    /// and launches the node.
    ///
    /// Returns a [NodeHandle] that can be used to interact with the node.
    pub async fn launch_node<N>(
        self,
        node: N,
    ) -> eyre::Result<
        NodeHandle<
            FullNodeComponentsAdapter<
                RethFullAdapter<DB, N>,
                <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
            >,
        >,
    >
    where
        N: Node<FullNodeTypesAdapter<N, DB, RethFullProviderType<DB, <N as NodeTypes>::Evm>>>,
        N::PoolBuilder: PoolBuilder<RethFullAdapter<DB, N>>,
        N::NetworkBuilder: crate::components::NetworkBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
        N::PayloadBuilder: crate::components::PayloadServiceBuilder<
            RethFullAdapter<DB, N>,
            <N::PoolBuilder as PoolBuilder<RethFullAdapter<DB, N>>>::Pool,
        >,
    {
        self.node(node).launch().await
    }
}

impl<DB, Types> WithLaunchContext<DB, TypesState<Types, DB>>
where
    Types: NodeTypes,
    DB: Database + Clone + Unpin + 'static,
{
    /// Configures the node's components.
    ///
    /// The given components builder is used to create the components of the node when it is
    /// launched.
    pub fn with_components<Components>(
        self,
        components_builder: Components,
    ) -> WithLaunchContext<
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
        WithLaunchContext {
            builder: self.builder.with_components(components_builder),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }
}

impl<DB, Types, Components>
    WithLaunchContext<
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
            builder: self.builder.map_components(f),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
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
            + Send
            + 'static,
    {
        self.builder.state.hooks.set_on_component_initialized(hook);
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
            + Send
            + 'static,
    {
        self.builder.state.hooks.set_on_node_started(hook);
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
            + Send
            + 'static,
    {
        self.builder.state.rpc.set_on_rpc_started(hook);
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
            + Send
            + 'static,
    {
        self.builder.state.rpc.set_extend_rpc_modules(hook);
        self
    }

    /// Launches the node and returns a handle to it.
    pub async fn launch(
        self,
    ) -> eyre::Result<
        NodeHandle<
            FullNodeComponentsAdapter<
                FullNodeTypesAdapter<Types, DB, RethFullProviderType<DB, Types::Evm>>,
                Components::Pool,
            >,
        >,
    > {
        let Self { builder, task_executor, data_dir } = self;

        builder.launch(task_executor, data_dir).await
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

    /// Loads `MAINNET_KZG_TRUSTED_SETUP`.
    pub fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
    }

    /// Returns the config for payload building.
    pub fn payload_builder_config(&self) -> impl PayloadBuilderConfig {
        self.config.builder.clone()
    }

    /// Returns the default network config for the node.
    pub fn network_config(&self) -> eyre::Result<NetworkConfig<Node::Provider>> {
        self.config.network_config(
            &self.reth_config,
            self.provider.clone(),
            self.executor.clone(),
            self.head,
            self.data_dir(),
        )
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

    /// Convenience function to start the network.
    ///
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
            .transactions(pool, Default::default())
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
