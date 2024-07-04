//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use std::{fmt, future::Future};

use derive_more::Constructor;
use reth_exex::ExExContext;
use reth_network::NetworkHandle;
use reth_node_api::{
    EngineComponent, FullNodeComponents, FullNodeComponentsExt, FullNodeTypes, NodeTypes,
    PipelineComponent, RpcComponent,
};
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;

use crate::{
    components::{NodeComponents, NodeComponentsBuilder},
    exex::BoxedLaunchExEx,
    hooks::NodeHooks,
    launch::LaunchNode,
    rpc::{RethRpcServerHandles, RpcContext, RpcHooks},
    FullNode,
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
    pub fn with_components<CB, R>(
        self,
        components_builder: CB,
    ) -> NodeBuilderWithComponents<T, CB, R>
    where
        CB: NodeComponentsBuilder<T>,
    {
        let Self { config, adapter } = self;

        NodeBuilderWithComponents {
            config,
            adapter,
            components_builder,
            add_ons: NodeAddOns {
                hooks: NodeHooks::default(),
                exexs: Vec::new(),
                hooks_ext: NodeAddOnsExt::new(RpcHooks::new()),
            },
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
pub struct NodeBuilderWithComponents<T: FullNodeTypes, CB: NodeComponentsBuilder<T>> {
    /// All settings for how the node should be configured.
    pub(crate) config: NodeConfig,
    /// Adapter for the underlying node types and database
    pub(crate) adapter: NodeTypesAdapter<T>,
    /// container for type specific components
    pub(crate) components_builder: CB,
    /// Additional node extensions.
    pub(crate) add_ons: NodeAddOns<NodeAdapter<T, CB::Components>>,
}

impl<T: FullNodeTypes, CB: NodeComponentsBuilder<T>> NodeBuilderWithComponents<T, CB> {
    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(NodeAdapter<T, CB::Components>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.hooks = self.add_ons.hooks.on_components_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(FullNode<NodeAdapter<T, CB::Components>>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.hooks.set_on_node_started(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<T, CB::Components>>,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.add_ons.rpc.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, NodeAdapter<T, CB::Components>>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.add_ons.rpc.set_extend_rpc_modules(hook);
        self
    }

    /// Installs an `ExEx` (Execution Extension) in the node.
    ///
    /// # Note
    ///
    /// The `ExEx` ID must be unique.
    pub fn install_exex<F, R, E>(mut self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: FnOnce(ExExContext<NodeAdapter<T, CB::Components>>) -> R + Send + 'static,
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
}

impl<T: FullNodeTypes, CB: NodeComponentsBuilder<T>> NodeBuilderWithComponents<T, CB> {
    /// Launches the node with the given launcher.
    pub async fn launch_with<L, C>(self, launcher: L) -> eyre::Result<L::Node>
    where
        L: LaunchNode<Self>,
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
    /// Hooks for optional components.
    pub(crate) hooks_ext: NodeAddOnsExt<Node>,
}

#[derive(Constructor)]
pub struct NodeAddOnsExt<Node: FullNodeComponents> {
    /// Additional RPC hooks.
    pub rpc: RpcHooks<Node>,
}

#[derive(Debug, Clone)]
pub struct NodeAdapterExt<
    Node: FullNodeComponents,
    Engine: EngineComponent<Self>,
    Rpc: RpcComponent<Self>,
> {
    pub core: Node,
    pub engine: Option<Engine>,
    //pub pipeline: Option<Pipeline>,
    pub rpc: Option<Rpc>,
}

impl<Node, Engine, Rpc> FullNodeComponents for NodeAdapterExt<Node, Engine, Rpc>
where
    Node: FullNodeComponents,
    Engine: EngineComponent<Self>,
    Rpc: RpcComponent<Self>,
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

    fn provider(&self) -> &Self::Provider {
        self.core.provider()
    }

    fn network(&self) -> &NetworkHandle {
        self.core.network()
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::EngineTypes> {
        self.core.payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        self.core.task_executor()
    }
}

impl<T, C, Engine, Rpc> FullNodeComponentsExt for NodeAdapterExt<NodeAdapter<T, C>, Engine, Rpc>
where
    T: FullNodeTypes,
    C: NodeComponents<T>,
    //Pipeline: PipelineComponent<Self>,
    Engine: EngineComponent<Self>,
    Rpc: RpcComponent<Self>,
{
    // type Pipeline = Pipeline;
    type Engine = Option<Engine>;
    type Rpc = Option<Rpc>;

    /*fn pipeline(&self) -> Option<&Self::Pipeline> {
        self.pipeline.as_ref()
    }*/

    fn engine(&self) -> Option<&<Self as FullNodeComponentsExt>::Engine> {
        self.engine.as_ref()
    }

    fn rpc(&self) -> Option<&Self::Rpc> {
        self.rpc.as_ref()
    }
}
