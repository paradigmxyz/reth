//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use crate::{
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::NodeHooks,
    launch::LaunchNode,
    rpc::{RethRpcAddOns, RethRpcServerHandles, RpcContext},
    AddOns, FullNode,
};
use reth_exex::ExExContext;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeAddOns, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_tasks::TaskExecutor;
use std::{fmt, future::Future};

/// A node builder that also has the configured types.
pub struct NodeBuilderWithTypes<T: FullNodeTypes + NodeTypes> {
    /// All settings for how the node should be configured.
    config: NodeConfig<<T::Types as NodeTypes>::ChainSpec>,
    /// The configured database for the node.
    adapter: NodeTypesAdapter<T>,
}

impl<T> NodeBuilderWithTypes<T>
where
    T: FullNodeTypes<Types = T> + NodeTypes,
{
    /// Creates a new instance of the node builder with the given configuration and types.
    pub const fn new(
        config: NodeConfig<<T::Types as NodeTypes>::ChainSpec>,
        database: T::DB,
    ) -> Self {
        Self { config, adapter: NodeTypesAdapter::new(database) }
    }

    /// Advances the state of the node builder to the next state where all components are configured
    pub fn with_components<CB>(self, components_builder: CB) -> NodeBuilderWithComponents<T, CB, ()>
    where
        CB: NodeComponentsBuilder<T>,
    {
        let Self { config, adapter } = self;

        NodeBuilderWithComponents {
            adapter: BuilderComponentsAdapter {
                config,
                database: adapter.database,
                components_builder,
                add_ons: AddOns { hooks: NodeHooks::default(), exexs: Vec::new(), add_ons: () },
            },
        }
    }
}

/// Container for the node's types and the database the node uses.
pub struct NodeTypesAdapter<T: FullNodeTypes> {
    /// The database type used by the node.
    pub database: T::DB,
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

/// Container for the node's types and the components and other internals that can be used by
/// addons of the node.
pub struct NodeAdapter<T: FullNodeTypes, C: NodeComponents<T>> {
    /// The components of the node.
    pub components: C,
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The provider of the node.
    pub provider: T::Provider,
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeTypes for NodeAdapter<T, C> {
    type Types = T::Types;
    type DB = T::DB;
    type Provider = T::Provider;
}

impl<T: FullNodeTypes, C: NodeComponents<T>> FullNodeComponents for NodeAdapter<T, C> {
    type Pool = C::Pool;
    type Evm = C::Evm;
    type Executor = C::Executor;
    type Consensus = C::Consensus;
    type Network = C::Network;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.components.evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.components.block_executor()
    }

    fn consensus(&self) -> &Self::Consensus {
        self.components.consensus()
    }

    fn network(&self) -> &Self::Network {
        self.components.network()
    }

    fn payload_builder_handle(
        &self,
    ) -> &reth_payload_builder::PayloadBuilderHandle<
        <Self::Types as reth_node_api::NodeTypesWithEngine>::Engine,
    > {
        self.components.payload_builder_handle()
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
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
    T: FullNodeTypes + NodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
> {
    pub adapter: BuilderComponentsAdapter<T, CB, AO>,
}

impl<T, CB> NodeBuilderWithComponents<T, CB, ()>
where
    T: FullNodeTypes + NodeTypes,
    CB: NodeComponentsBuilder<T>,
{
    /// Advances the state of the node builder to the next state where all customizable
    /// [`NodeAddOns`] types are configured.
    pub fn with_add_ons<AO>(self, add_ons: AO) -> NodeBuilderWithComponents<T, CB, AO>
    where
        AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
    {
        let Self { adapter } = self;

        NodeBuilderWithComponents {
            adapter: BuilderComponentsAdapter {
                config: adapter.config,
                database: adapter.database,
                components_builder: adapter.components_builder,
                add_ons: AddOns { hooks: NodeHooks::default(), exexs: Vec::new(), add_ons },
            },
        }
    }
}

impl<T, CB, AO> NodeBuilderWithComponents<T, CB, AO>
where
    T: FullNodeTypes + NodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
{
    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(NodeAdapter<T, CB::Components>) -> eyre::Result<()> + Send + 'static,
    {
        self.adapter.add_ons.hooks.set_on_component_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(FullNode<NodeAdapter<T, CB::Components>, AO>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.adapter.add_ons.hooks.set_on_node_started(hook);
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
        self.adapter.add_ons.exexs.push((exex_id.into(), Box::new(exex)));
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

    /// Modifies the addons with the given closure.
    pub fn map_add_ons<F>(mut self, f: F) -> Self
    where
        F: FnOnce(AO) -> AO,
    {
        self.adapter.add_ons.add_ons = f(self.adapter.add_ons.add_ons);
        self
    }
}

impl<T, CB, AO> NodeBuilderWithComponents<T, CB, AO>
where
    T: FullNodeTypes + NodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: RethRpcAddOns<NodeAdapter<T, CB::Components>>,
{
    /// Launches the node with the given launcher.
    pub async fn launch_with<L>(self, launcher: L) -> eyre::Result<L::Node>
    where
        L: LaunchNode<Self>,
    {
        launcher.launch_node(self).await
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(self, hook: F) -> Self
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.map_add_ons(|mut add_ons| {
            add_ons.hooks_mut().set_on_rpc_started(hook);
            add_ons
        })
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.map_add_ons(|mut add_ons| {
            add_ons.hooks_mut().set_extend_rpc_modules(hook);
            add_ons
        })
    }
}

trait BuilderInternals {
    /// The node types that this builder works with
    type Types: FullNodeTypes + NodeTypes;

    /// The components that will be built
    type Components: NodeComponents<Self::Types>;

    /// The add-ons that will be attached to the node
    type AddOns: NodeAddOns<NodeAdapter<Self::Types, Self::Components>>;

    /// The components builder type
    type ComponentsBuilder: NodeComponentsBuilder<Self::Types, Components = Self::Components>
        + Clone;
}

/// An adapter that manages the components and configuration for building a node.
pub struct BuilderComponentsAdapter<T, C, AO = ()>
where
    T: FullNodeTypes + NodeTypes,
    C: NodeComponentsBuilder<T>,
    AO: NodeAddOns<NodeAdapter<T, C::Components>>,
{
    /// The node configuration
    config: NodeConfig<<T as NodeTypes>::ChainSpec>,
    /// The database
    database: T::DB,
    /// The components builder
    components_builder: C,
    /// The add-ons for the node
    add_ons: AddOns<NodeAdapter<T, C::Components>, AO>,
}

impl<T, C, AO> BuilderComponentsAdapter<T, C, AO>
where
    T: FullNodeTypes + NodeTypes,
    C: NodeComponentsBuilder<T> + Clone,
    AO: NodeAddOns<NodeAdapter<T, C::Components>>,
{
    /// Returns a reference to the node configuration
    pub fn config(&self) -> &NodeConfig<<T as NodeTypes>::ChainSpec> {
        &self.config
    }

    /// Returns a reference to the database
    pub fn database(&self) -> &T::DB {
        &self.database
    }

    /// Returns a reference to the components builder
    pub fn components_builder(&self) -> &C {
        &self.components_builder
    }

    /// Returns a mutable reference to the add-ons.
    ///
    /// This method is intended to be used only by the builder implementation
    /// for managing add-ons state during the node building process.
    pub fn add_ons_mut(&mut self) -> &mut AddOns<NodeAdapter<T, C::Components>, AO> {
        &mut self.add_ons
    }
}

impl<T, C, AO> BuilderInternals for BuilderComponentsAdapter<T, C, AO>
where
    T: FullNodeTypes + NodeTypes,
    C: NodeComponentsBuilder<T> + Clone,
    AO: NodeAddOns<NodeAdapter<T, C::Components>>,
{
    type Types = T;
    type Components = C::Components;
    type AddOns = AO;
    type ComponentsBuilder = C;
}
