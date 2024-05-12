//! Customizable node builder.

#![allow(clippy::type_complexity, missing_debug_implementations)]

use crate::{
    components::NodeComponentsBuilder,
    node::FullNode,
    rpc::{RethRpcServerHandles, RpcContext},
    DefaultNodeLauncher, Node, NodeHandle,
};
use futures::Future;
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_exex::ExExContext;
use reth_network::{NetworkBuilder, NetworkConfig, NetworkHandle};
use reth_node_api::{FullNodeTypes, FullNodeTypesAdapter, NodeTypes};
use reth_node_core::{
    cli::config::{PayloadBuilderConfig, RethTransactionPoolConfig},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
    node_config::NodeConfig,
    primitives::{kzg::KzgSettings, Head},
    utils::write_peers_to_file,
};
use reth_primitives::{constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, ChainSpec};
use reth_provider::{providers::BlockchainProvider, ChainSpecProvider};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{PoolConfig, TransactionPool};
pub use states::*;
use std::{str::FromStr, sync::Arc};

mod states;

/// The adapter type for a reth node with the builtin provider type
// Note: we need to hardcode this because custom components might depend on it in associated types.
pub type RethFullAdapter<DB, Types> = FullNodeTypesAdapter<Types, DB, BlockchainProvider<DB>>;

#[cfg_attr(doc, aquamarine::aquamarine)]
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
/// Next all stateful components of the node are configured, these include all the
/// components of the node that are downstream of those types, these include:
///
///  - The EVM and Executor configuration: [ExecutorBuilder](crate::components::ExecutorBuilder)
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
/// creating the node components during the launch process. The
/// [ComponentsBuilder](crate::components::ComponentsBuilder) is a general purpose implementation of
/// the [NodeComponentsBuilder] trait that can be used to configure the executor, network,
/// transaction pool and payload builder of the node. It enforces the correct order of
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
/// ## Flow
///
/// The [NodeBuilder] is intended to sit behind a CLI that provides the necessary [NodeConfig]
/// input: [NodeBuilder::new]
///
/// From there the builder is configured with the node's types, components, and hooks, then launched
/// with the [NodeBuilder::launch] method. On launch all the builtin internals, such as the
/// `Database` and its providers [BlockchainProvider] are initialized before the configured
/// [NodeComponentsBuilder] is invoked with the [BuilderContext] to create the transaction pool,
/// network, and payload builder components. When the RPC is configured, the corresponding hooks are
/// invoked to allow for custom rpc modules to be injected into the rpc server:
/// [NodeBuilder::extend_rpc_modules]
///
/// Finally all components are created and all services are launched and a [NodeHandle] is returned
/// that can be used to interact with the node: [FullNode]
///
/// The following diagram shows the flow of the node builder from CLI to a launched node.
///
/// include_mmd!("docs/mermaid/builder.mmd")
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
/// specified above. It also expects beacon consensus with the ethereum engine API that is
/// configured by the builder itself during launch. This might change in the future.
///
/// [builder]: https://doc.rust-lang.org/1.0.0/style/ownership/builders.html
pub struct NodeBuilder<DB> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// The configured database for the node.
    database: DB,
}

impl NodeBuilder<()> {
    /// Create a new [`NodeBuilder`].
    pub fn new(config: NodeConfig) -> Self {
        Self { config, database: () }
    }
}

impl<DB> NodeBuilder<DB> {
    /// Returns a reference to the node builder's config.
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Configures the underlying database that the node will use.
    pub fn with_database<D>(self, database: D) -> NodeBuilder<D> {
        NodeBuilder { config: self.config, database }
    }

    /// Preconfigure the builder with the context to launch the node.
    ///
    /// This provides the task executor and the data directory for the node.
    pub fn with_launch_context(
        self,
        task_executor: TaskExecutor,
        data_dir: ChainPath<DataDirPath>,
    ) -> WithLaunchContext<NodeBuilder<DB>> {
        WithLaunchContext { builder: self, task_executor, data_dir }
    }

    /// Creates an _ephemeral_ preconfigured node for testing purposes.
    pub fn testing_node(
        self,
        task_executor: TaskExecutor,
    ) -> WithLaunchContext<NodeBuilder<Arc<TempDatabase<DatabaseEnv>>>> {
        let db = create_test_rw_db();
        let db_path_str = db.path().to_str().expect("Path is not valid unicode");
        let path =
            MaybePlatformPath::<DataDirPath>::from_str(db_path_str).expect("Path is not valid");
        let data_dir = path.unwrap_or_chain_default(self.config.chain.chain);

        WithLaunchContext { builder: self.with_database(db), task_executor, data_dir }
    }
}

impl<DB> NodeBuilder<DB>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    /// Configures the types of the node.
    pub fn with_types<T>(self) -> NodeBuilderWithTypes<RethFullAdapter<DB, T>>
    where
        T: NodeTypes,
    {
        NodeBuilderWithTypes::new(self.config, self.database)
    }

    /// Preconfigures the node with a specific node implementation.
    ///
    /// This is a convenience method that sets the node's types and components in one call.
    pub fn node<N>(
        self,
        node: N,
    ) -> NodeBuilderWithComponents<RethFullAdapter<DB, N>, N::ComponentsBuilder>
    where
        N: Node<RethFullAdapter<DB, N>>,
    {
        self.with_types().with_components(node.components_builder())
    }
}

/// A [NodeBuilder] with it's launch context already configured.
///
/// This exposes the same methods as [NodeBuilder] but with the launch context already configured,
/// See [WithLaunchContext::launch]
pub struct WithLaunchContext<Builder> {
    builder: Builder,
    task_executor: TaskExecutor,
    data_dir: ChainPath<DataDirPath>,
}

impl<Builder> WithLaunchContext<Builder> {
    /// Returns a reference to the task executor.
    pub fn task_executor(&self) -> &TaskExecutor {
        &self.task_executor
    }

    /// Returns a reference to the data directory.
    pub fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.data_dir
    }
}

impl<DB> WithLaunchContext<NodeBuilder<DB>>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    /// Returns a reference to the node builder's config.
    pub fn config(&self) -> &NodeConfig {
        self.builder.config()
    }

    /// Configures the types of the node.
    pub fn with_types<T>(self) -> WithLaunchContext<NodeBuilderWithTypes<RethFullAdapter<DB, T>>>
    where
        T: NodeTypes,
    {
        WithLaunchContext {
            builder: self.builder.with_types(),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Preconfigures the node with a specific node implementation.
    ///
    /// This is a convenience method that sets the node's types and components in one call.
    pub fn node<N>(
        self,
        node: N,
    ) -> WithLaunchContext<NodeBuilderWithComponents<RethFullAdapter<DB, N>, N::ComponentsBuilder>>
    where
        N: Node<RethFullAdapter<DB, N>>,
    {
        self.with_types().with_components(node.components_builder())
    }

    /// Launches a preconfigured [Node]
    ///
    /// This bootstraps the node internals, creates all the components with the given [Node]
    ///
    /// Returns a [NodeHandle] that can be used to interact with the node.
    pub async fn launch_node<N>(
        self,
        node: N,
    ) -> eyre::Result<
        NodeHandle<
            NodeAdapter<
                RethFullAdapter<DB, N>,
                <N::ComponentsBuilder as NodeComponentsBuilder<RethFullAdapter<DB, N>>>::Components,
            >,
        >,
    >
    where
        N: Node<RethFullAdapter<DB, N>>,
    {
        self.node(node).launch().await
    }
}

impl<T, DB> WithLaunchContext<NodeBuilderWithTypes<RethFullAdapter<DB, T>>>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
    T: NodeTypes,
{
    /// Advances the state of the node builder to the next state where all components are configured
    pub fn with_components<CB>(
        self,
        components_builder: CB,
    ) -> WithLaunchContext<NodeBuilderWithComponents<RethFullAdapter<DB, T>, CB>>
    where
        CB: NodeComponentsBuilder<RethFullAdapter<DB, T>>,
    {
        WithLaunchContext {
            builder: self.builder.with_components(components_builder),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }
}

impl<T, DB, CB> WithLaunchContext<NodeBuilderWithComponents<RethFullAdapter<DB, T>, CB>>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
    T: NodeTypes,
    CB: NodeComponentsBuilder<RethFullAdapter<DB, T>>,
{
    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(self, hook: F) -> Self
    where
        F: FnOnce(NodeAdapter<RethFullAdapter<DB, T>, CB::Components>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        Self {
            builder: self.builder.on_component_initialized(hook),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(self, hook: F) -> Self
    where
        F: FnOnce(
                FullNode<NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        Self {
            builder: self.builder.on_node_started(hook),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(self, hook: F) -> Self
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        Self {
            builder: self.builder.on_rpc_started(hook),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(self, hook: F) -> Self
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        Self {
            builder: self.builder.extend_rpc_modules(hook),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Installs an ExEx (Execution Extension) in the node.
    ///
    /// # Note
    ///
    /// The ExEx ID must be unique.
    pub fn install_exex<F, R, E>(self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: FnOnce(ExExContext<NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>) -> R
            + Send
            + 'static,
        R: Future<Output = eyre::Result<E>> + Send,
        E: Future<Output = eyre::Result<()>> + Send,
    {
        Self {
            builder: self.builder.install_exex(exex_id, exex),
            task_executor: self.task_executor,
            data_dir: self.data_dir,
        }
    }

    /// Launches the node and returns a handle to it.
    pub async fn launch(
        self,
    ) -> eyre::Result<NodeHandle<NodeAdapter<RethFullAdapter<DB, T>, CB::Components>>> {
        let Self { builder, task_executor, data_dir } = self;

        let launcher = DefaultNodeLauncher::new(task_executor, data_dir);
        builder.launch_with(launcher).await
    }

    /// Check that the builder can be launched
    ///
    /// This is useful when writing tests to ensure that the builder is configured correctly.
    pub fn check_launch(self) -> Self {
        self
    }
}

/// Captures the necessary context for building the components of the node.
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    pub(crate) head: Head,
    /// The configured provider to interact with the blockchain.
    pub(crate) provider: Node::Provider,
    /// The executor of the node.
    pub(crate) executor: TaskExecutor,
    /// The data dir of the node.
    pub(crate) data_dir: ChainPath<DataDirPath>,
    /// The config of the node
    pub(crate) config: NodeConfig,
    /// loaded config
    pub(crate) reth_config: reth_config::Config,
}

impl<Node: FullNodeTypes> BuilderContext<Node> {
    /// Create a new instance of [BuilderContext]
    pub fn new(
        head: Head,
        provider: Node::Provider,
        executor: TaskExecutor,
        data_dir: ChainPath<DataDirPath>,
        config: NodeConfig,
        reth_config: reth_config::Config,
    ) -> Self {
        Self { head, provider, executor, data_dir, config, reth_config }
    }

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

        let default_peers_path = self.data_dir().known_peers();
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

impl<Node: FullNodeTypes> std::fmt::Debug for BuilderContext<Node> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuilderContext")
            .field("head", &self.head)
            .field("provider", &std::any::type_name::<Node::Provider>())
            .field("executor", &self.executor)
            .field("data_dir", &self.data_dir)
            .field("config", &self.config)
            .finish()
    }
}
