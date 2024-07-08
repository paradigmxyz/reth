//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use std::{fmt, future::Future, pin::Pin};

use derive_more::{Constructor, Deref};
use futures::future::Either;
use reth_auto_seal_consensus::AutoSealClient;
use reth_beacon_consensus::FullBlockchainTreeEngine;
use reth_exex::ExExContext;
use reth_network::{FetchClient, NetworkHandle};
use reth_network_p2p::FullClient;
use reth_node_api::{FullNodeComponents, FullNodeComponentsExt, FullNodeTypes, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::providers::BlockchainProvider;
use reth_tasks::TaskExecutor;

use crate::{
    common::ExtBuilderContext,
    components::{NodeComponents, NodeComponentsBuilder},
    exex::BoxedLaunchExEx,
    hooks::NodeHooks,
    launch::LaunchNode,
    rpc::RpcHooks,
    EngineAdapter, FullNode, PipelineAdapter, RpcAdapter,
};

/// A node builder that also has the configured types.
pub struct NodeBuilderWithTypes<T: FullNodeTypes> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// The configured database for the node.
    adapter: NodeTypesAdapter<T>,
}

impl<T: FullNodeTypes> NodeBuilderWithTypes<T> {
    /// Creates a new instance of the node builder with the given configuration and types.
    pub const fn new(config: NodeConfig, database: T::DB) -> Self {
        Self { config, adapter: NodeTypesAdapter::new(database) }
    }

    /// Advances the state of the node builder to the next state where all components are configured
    pub fn with_components<CB>(self, components_builder: CB) -> NodeBuilderWithComponents<T, CB>
    where
        CB: NodeComponentsBuilder<T>,
    {
        let Self { config, adapter } = self;

        NodeBuilderWithComponents {
            config,
            adapter,
            components_builder,
            add_ons: NodeAddOns { hooks: NodeHooks::default(), exexs: Vec::new() },
        }
    }
}

/// Container for the node's types and the database the node uses.
pub(crate) struct NodeTypesAdapter<T: FullNodeTypes> {
    /// The database type used by the node.
    pub(crate) database: T::DB,
}

impl<T: FullNodeTypes> NodeTypesAdapter<T> {
    /// Create a new adapter from the given node types.
    pub(crate) const fn new(database: T::DB) -> Self {
        Self { database }
    }
}

impl<T: FullNodeTypes> fmt::Debug for NodeTypesAdapter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeTypesAdapter").field("db", &"...").field("types", &"...").finish()
    }
}

/// Container for the node's types and the components and other internals that can be used by addons
/// of the node.
pub struct NodeAdapter<T: FullNodeTypes, C: NodeComponents<T>> {
    /// The core components of the node.
    pub components: C,
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The provider of the node.
    pub provider: T::Provider,
}

impl<T: FullNodeTypes, C: NodeComponents<T>> NodeTypes for NodeAdapter<T, C> {
    type Primitives = T::Primitives;
    type EngineTypes = T::EngineTypes;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeTypes for NodeAdapter<T, C> {
    type DB = T::DB;
    type Provider = T::Provider;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeComponents for NodeAdapter<T, C> {
    type Pool = C::Pool;
    type Evm = C::Evm;
    type Executor = C::Executor;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.components.evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.components.block_executor()
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn network(&self) -> &NetworkHandle {
        self.components.network()
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<T::EngineTypes> {
        self.components.payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        &self.task_executor
    }
}

impl<T: FullNodeTypes, C: NodeComponents<T>> Clone for NodeAdapter<T, C> {
    fn clone(&self) -> Self {
        Self {
            components: self.components.clone(),
            task_executor: self.task_executor.clone(),
            provider: self.provider.clone(),
        }
    }
}

/// A fully type configured node builder.
///
/// Supports adding additional addons to the node.
pub struct NodeBuilderWithComponents<
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    Output: FullNodeComponentsExt = NodeAdapterExt<
        NodeAdapter<T, <CB as NodeComponentsBuilder<T>>::Components>,
        BlockchainProvider<<T as FullNodeTypes>::DB>,
        Either<AutoSealClient, FetchClient>,
    >,
> {
    /// All settings for how the node should be configured.
    pub(crate) config: NodeConfig,
    /// Adapter for the underlying node types and database
    pub(crate) adapter: NodeTypesAdapter<T>,
    /// container for type specific components
    pub(crate) components_builder: CB,
    /// Additional node extensions.
    pub(crate) add_ons: NodeAddOns<Output>,
}

impl<T, CB, Output> NodeBuilderWithComponents<T, CB, Output>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    Output: FullNodeComponentsExt,
{
    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_components_initialized<F>(mut self, hook: F) -> Self
    where
        for<'a> F: FnOnce(
                ExtBuilderContext<'a, Output>,
            ) -> Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>
            + Send
            + 'static,
    {
        self.add_ons.hooks = self.add_ons.hooks.on_components_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(FullNode<Output>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.hooks.set_on_node_started(hook);
        self
    }

    /// Installs an `ExEx` (Execution Extension) in the node.
    ///
    /// # Note
    ///
    /// The `ExEx` ID must be unique.
    pub fn install_exex<F, R, E>(mut self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: FnOnce(ExExContext<Output>) -> R + Send + 'static,
        R: Future<Output = eyre::Result<E>> + Send,
        E: Future<Output = eyre::Result<()>> + Send,
    {
        self.add_ons.exexs.push((exex_id.into(), Box::new(exex)));
        self
    }

    /// Launches the node with the given closure.
    pub fn launch_with_fn<L, R>(self, launcher: L) -> R
    where
        L: FnOnce(Self) -> R,
    {
        launcher(self)
    }

    /// Check that the builder can be launched
    ///
    /// This is useful when writing tests to ensure that the builder is configured correctly.
    pub const fn check_launch(self) -> Self {
        self
    }

    /// Launches the node with the given launcher.
    pub async fn launch_with<L>(self, launcher: L) -> eyre::Result<L::Node>
    where
        L: LaunchNode<Self, Node = Output>,
    {
        launcher.launch_node(self).await
    }
}

/// Additional node extensions.
pub(crate) struct NodeAddOns<Node: FullNodeComponents> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub(crate) hooks: NodeHooks<Node>,
    /// The `ExExs` (execution extensions) of the node.
    pub(crate) exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
}

#[allow(missing_debug_implementations)]
#[derive(Deref, Clone)]
pub struct NodeAdapterExt<
    Node: FullNodeComponents,
    BT: FullBlockchainTreeEngine + 'static,
    C: FullClient + 'static,
> {
    #[deref]
    pub core: Node,
    pub tree: Option<BlockchainProvider<Node::DB>>,
    pub pipeline: Option<PipelineAdapter<C>>,
    pub engine: Option<EngineAdapter<Node, BT, C>>,
    pub rpc: Option<RpcAdapter<Node>>,
}

impl<Node, BT, C> NodeAdapterExt<Node, BT, C>
where
    Node: FullNodeComponents,
    BT: FullBlockchainTreeEngine + 'static,
    C: FullClient + 'static,
{
    pub fn new(core: Node) -> Self {
        Self { core, tree: None, pipeline: None, engine: None, rpc: None }
    }
}

impl<Node, BT, C> FullNodeComponents for NodeAdapterExt<Node, BT, C>
where
    Node: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Clone + 'static,
    C: FullClient + Clone + 'static,
{
    type Pool = Node::Pool;
    type Evm = Node::Evm;
    type Executor = Node::Executor;

    fn pool(&self) -> &Self::Pool {
        self.core.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.core.evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.core.block_executor()
    }

    fn provider(&self) -> &<Self as FullNodeTypes>::Provider {
        self.core.provider()
    }

    fn network(&self) -> &NetworkHandle {
        self.core.network()
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Node::EngineTypes> {
        self.core.payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        self.core.task_executor()
    }
}

impl<N, BT, C> FullNodeComponentsExt for NodeAdapterExt<N, BT, C>
where
    N: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Send + Sync + Unpin + Clone + 'static,
    C: FullClient + Send + Sync + Clone + 'static,
{
    type Core = N;
    type Tree = BlockchainProvider<N::DB>;
    type Pipeline = PipelineAdapter<C>;
    type Engine = EngineAdapter<N, BT, C>;
    type Rpc = RpcAdapter<N>;

    fn from_core(core: Self::Core) -> Self {
        Self::new(core)
    }

    fn tree(&self) -> Option<&Self::Tree> {
        self.tree.as_ref()
    }

    fn pipeline(&self) -> Option<&Self::Pipeline> {
        self.pipeline.as_ref()
    }

    fn engine(&self) -> Option<&Self::Engine> {
        self.engine.as_ref()
    }

    fn rpc(&self) -> Option<&Self::Rpc> {
        self.rpc.as_ref()
    }

    fn into_core(self) -> Self::Core {
        self.core
    }
}

#[derive(Constructor)]
pub struct NodeAddOnsExt<Node: FullNodeComponents> {
    /// Additional RPC hooks.
    pub rpc: RpcHooks<Node>,
}
