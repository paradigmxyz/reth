//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use crate::{
    components::{NodeComponents, NodeComponentsBuilder},
    exex::BoxedLaunchExEx,
    hooks::NodeHooks,
    launch::LaunchNode,
    rpc::{RethRpcServerHandles, RpcContext, RpcHooks},
    FullNode,
};
use reth_exex::ExExContext;
use reth_network::NetworkHandle;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;
use std::{fmt, future::Future};

/// A node builder that also has the configured types.
pub struct NodeBuilderWithTypes<T: FullNodeTypes> {
    /// All settings for how the node should be configured.
    config: NodeConfig,
    /// The configured database for the node.
    adapter: NodeTypesAdapter<T>,
}

impl<T: FullNodeTypes> NodeBuilderWithTypes<T> {
    /// Creates a new instance of the node builder with the given configuration and types.
    pub fn new(config: NodeConfig, database: T::DB) -> Self {
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
    pub(crate) database: T::DB,
}

impl<T: FullNodeTypes> NodeTypesAdapter<T> {
    /// Create a new adapter from the given node types.
    pub(crate) fn new(database: T::DB) -> Self {
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
    /// The components of the node.
    pub components: C,
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The provider of the node.
    pub provider: T::Provider,
}

impl<T: FullNodeTypes, C: NodeComponents<T>> NodeTypes for NodeAdapter<T, C> {
    type Primitives = T::Primitives;
    type Engine = T::Engine;
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

    fn payload_builder(&self) -> &PayloadBuilderHandle<T::Engine> {
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
        self.add_ons.hooks.set_on_component_initialized(hook);
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

    /// Installs an ExEx (Execution Extension) in the node.
    ///
    /// # Note
    ///
    /// The ExEx ID must be unique.
    pub fn install_exex<F, R, E>(mut self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: FnOnce(ExExContext<NodeAdapter<T, CB::Components>>) -> R + Send + 'static,
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

    /// Check that the builder can be launched
    ///
    /// This is useful when writing tests to ensure that the builder is configured correctly.
    pub fn check_launch(self) -> Self {
        self
    }
}

/// Additional node extensions.
pub(crate) struct NodeAddOns<Node: FullNodeComponents> {
    /// Additional NodeHooks that are called at specific points in the node's launch lifecycle.
    pub(crate) hooks: NodeHooks<Node>,
    /// Additional RPC hooks.
    pub(crate) rpc: RpcHooks<Node>,
    /// The ExExs (execution extensions) of the node.
    pub(crate) exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
}
