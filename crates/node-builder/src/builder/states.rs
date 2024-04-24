//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use reth_exex::ExExContext;
use reth_network::NetworkHandle;
use std::{future::Future, marker::PhantomData};
use reth_node_api::{
    FullNodeComponents, FullNodeComponentsAdapter, FullNodeTypes, FullNodeTypesAdapter, NodeTypes,
};
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;

use crate::{
    builder::RethFullProviderType,
    exex::BoxedLaunchExEx,
    hooks::NodeHooks,
    launch::LaunchNode,
    rpc::{RethRpcServerHandles, RpcContext, RpcHooks},
    BuilderContext, FullNode, NodeHandle,
};
use crate::components::{NodeComponents, NodeComponentsBuilder};

/// A node builder that also has the configured types.
pub struct NodeBuilderWithTypes<T: FullNodeTypes> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// The configured database for the node.
    adapter: NodeTypesAdapter<T>,
}

impl<T: FullNodeTypes> NodeBuilderWithTypes<T> {
    /// Creates a new instance of the node builder with the given configuration and types.
    pub fn new(config: NodeConfig, types: T, db: T::DB) -> Self {
        Self { config, adapter: NodeTypesAdapter::new(types, db) }
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
            add_ons: NodeAddOns {
                hooks: NodeHooks::default(),
                rpc: RpcHooks::new(),
                exexs: Vec::new(),
            },
        }
    }
}

/// Container for the node's types and the database the node uses.
pub(crate) struct NodeTypesAdapter<T: FullNodeTypes> {
    /// The database type used by the node.
    pub(crate) db: T::DB,
    // TODO(mattsse): make this stateless
    pub(crate) types: T,
}

impl<T: FullNodeTypes> NodeTypesAdapter<T> {
    /// Create a new adapter from the given node types.
    pub fn new(types: T, db: T::DB) -> Self {
        Self { types, db }
    }
}

/// Container for the node's types and the components and other internals that can be used by addons
/// of the node.
pub struct NodeAdapter<T: FullNodeTypes, C: NodeComponents<T>> {
    pub components: C,
    pub task_executor: TaskExecutor,
    pub provider: T::Provider,
}

impl<T: FullNodeTypes, C: NodeComponents<T>> NodeTypes for NodeAdapter<T, C> {
    type Primitives = T::Primitives;
    type Engine = T::Engine;
    type Evm = T::Evm;

    fn evm_config(&self) -> Self::Evm {
        // TODO(mattsse): make this stateless
        todo!()
    }
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeTypes for NodeAdapter<T, C> {
    type DB = T::DB;
    type Provider = T::Provider;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeComponents for NodeAdapter<T, C> {
    type Pool = C::Pool;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn network(&self) -> &NetworkHandle {
        self.components.network()
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<T::Engine> {
        self.components.payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        &self.task_executor
    }
}

/// A fully type configured node builder.
///
/// Supports adding additional addons to the node.
pub struct NodeBuilderWithComponents<T: FullNodeTypes, CB: NodeComponentsBuilder<T>> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// Adapter for the underlying node types and database
    adapter: NodeTypesAdapter<T>,
    /// container for type specific components
    components_builder: CB,
    /// Additional node extensions.
    add_ons: NodeAddOns<NodeAdapter<T, CB::Components>>,
}

impl<T: FullNodeTypes, CB: NodeComponentsBuilder<T>> NodeBuilderWithComponents<T, CB> {
    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: Fn(NodeAdapter<T, CB::Components>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.hooks.set_on_component_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: Fn(FullNode<NodeAdapter<T, CB::Components>>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.hooks.set_on_node_started(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: Fn(
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
        F: Fn(RpcContext<'_, NodeAdapter<T, CB::Components>>) -> eyre::Result<()> + Send + 'static,
    {
        self.add_ons.rpc.set_extend_rpc_modules(hook);
        self
    }

    /// Installs an ExEx (Execution Extension) in the node.
    ///
    /// # Note
    ///
    /// The ExEx ID must be unique.
    pub fn install_exex<F, R, E>(mut self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: Fn(ExExContext<NodeAdapter<T, CB::Components>>) -> R + Send + 'static,
        R: Future<Output = eyre::Result<E>> + Send,
        E: Future<Output = eyre::Result<()>> + Send,
    {
        self.add_ons.exexs.push((exex_id.into(), Box::new(exex)));
        self
    }

    /// Launches the node with the given launcher.
    pub async fn launch_with<L>(self, launcher: L) -> eyre::Result<L::Node>
    where
        L: LaunchNode<Self>,
    {
        launcher.launch_node(self).await
    }
}

/// Additional node extensions.
struct NodeAddOns<Node: FullNodeComponents> {
    /// Additional NodeHooks that are called at specific points in the node's launch lifecycle.
    hooks: NodeHooks<Node>,
    /// Additional RPC hooks.
    rpc: RpcHooks<Node>,
    /// The ExExs (execution extensions) of the node.
    exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
}
