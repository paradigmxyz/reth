//! Customizable node builder.

use crate::{components::NodeComponentsBuilder, node::FullNodeTypesAdapter, NodeHandle};
use reth_blockchain_tree::ShareableBlockchainTree;
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_node_api::node::{FullNodeTypes, NodeTypes};
use reth_node_core::{node_config::NodeConfig, primitives::Head};
use reth_provider::providers::BlockchainProvider;
use reth_revm::EvmProcessorFactory;
use reth_tasks::TaskExecutor;
use std::{marker::PhantomData, sync::Arc};

/// The builtin provider type of the reth node.
// Note: we need to hardcode this because custom components might depend on it in associated types.
// TODO: this will eventually depend on node primitive types and evm
type RethFullProviderType<DB> =
    BlockchainProvider<Arc<DB>, ShareableBlockchainTree<Arc<DB>, EvmProcessorFactory>>;

/// Declaratively construct a node.
///
/// [`NodeBuilder`] provides a [builder-like interface][builder] for composing
/// components of a node.
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
    DB: Database,
{
    /// Configures the types of the node.
    pub fn with_types<T>(self) -> NodeBuilder<DB, TypesState<T, DB>> {
        NodeBuilder { config: self.config, state: Default::default(), database: self.database }
    }
}

impl<DB, Types> NodeBuilder<DB, TypesState<Types, DB>>
where
    Types: NodeTypes,
    DB: Database + 'static,
{
    /// Configures the node's components.
    pub fn with_components<Builder>(
        self,
        builder: Builder,
    ) -> NodeBuilder<DB, ComponentsState<Types, Builder>>
    where
        Builder: NodeComponentsBuilder<FullNodeTypesAdapter<Types, RethFullProviderType<DB>>>,
    {
        NodeBuilder {
            config: self.config,
            database: self.database,
            state: ComponentsState { _maker: Default::default(), builder },
        }
    }
}

impl<DB, Types, Components> NodeBuilder<DB, ComponentsState<Types, Components>>
where
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + 'static,
    Types: NodeTypes,
    Components: NodeComponentsBuilder<FullNodeTypesAdapter<Types, RethFullProviderType<DB>>>,
{
    /// Launches the node and returns a handle to it.
    pub async fn launch(self, _executor: TaskExecutor) -> eyre::Result<NodeHandle> {
        // 1. create the `BuilderContext`
        // 2. build the components
        // 3. build/customize rpc
        // 4. apply hooks

        todo!()
    }
}

/// Captures the necessary context for building the components of the node.
#[derive(Debug)]
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    head: Head,
    /// The configured provider to interact with the blockchain.
    provider: Node::Provider,

    executor: TaskExecutor,

    // TODO maybe combine this with provider
    events: (),

    /// The data dir of the node.
    data_dir: (),
    /// The config of the node
    config: NodeConfig,
}

impl<Node: FullNodeTypes> BuilderContext<Node> {
    pub fn provider(&self) -> &Node::Provider {
        &self.provider
    }

    /// Returns the current head of the blockchain at launch.
    pub fn head(&self) -> Head {
        self.head
    }

    /// Returns the data dir of the node.
    pub fn data_dir(&self) -> &() {
        &self.data_dir
    }

    // TODO read only helper methods to access the config traits (cli args)
}

/// The initial state of the node builder process.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct InitState;

/// The state after all types of the node have been configured.
#[derive(Debug)]
pub struct TypesState<Types, DB>
where
    DB: Database,
{
    adapter: FullNodeTypesAdapter<Types, RethFullProviderType<DB>>,
}

impl<Types, DB> Default for TypesState<Types, DB>
where
    DB: Database,
{
    fn default() -> Self {
        Self { adapter: Default::default() }
    }
}

/// The state of the node builder process after the node's components have been configured.
///
/// With this state all types and components of the node are known and the node can be launched.
#[derive(Debug)]
pub struct ComponentsState<Types, Builder> {
    _maker: PhantomData<Types>,
    builder: Builder,
}
