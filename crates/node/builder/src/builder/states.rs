//! Node builder states and helper traits.
//!
//! Keeps track of the current state of the node builder.
//!
//! The node builder process is essentially a state machine that transitions through various states
//! before the node can be launched.

use crate::{
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::{NodeHooks, OnNodeStartedHook, RpcHooks},
    launch::LaunchNode,
    rpc::{RethRpcAddOns, RethRpcServerHandles, RpcContext},
    AddOns, ComponentsFor, FullNode,
};

use reth_exex::ExExContext;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeAddOns, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_tasks::TaskExecutor;
use std::{fmt, fmt::Debug, future::Future};

/// A node builder that also has the configured types.
pub struct NodeBuilderWithTypes<T: FullNodeTypes> {
    /// All settings for how the node should be configured.
    config: NodeConfig<<T::Types as NodeTypes>::ChainSpec>,
    /// The configured database for the node.
    adapter: NodeTypesAdapter<T>,
}

impl<T: FullNodeTypes> NodeBuilderWithTypes<T> {
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
            config,
            adapter,
            components_builder,
            add_ons: AddOns { hooks: NodeHooks::default(), exexs: Vec::new(), add_ons: () },
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
#[derive(Debug)]
pub struct NodeAdapter<T: FullNodeTypes, C: NodeComponents<T> = ComponentsFor<T>> {
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
    type Consensus = C::Consensus;
    type Network = C::Network;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.components.evm_config()
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
        <Self::Types as reth_node_api::NodeTypes>::Payload,
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
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
> {
    /// All settings for how the node should be configured.
    pub config: NodeConfig<<T::Types as NodeTypes>::ChainSpec>,
    /// Adapter for the underlying node types and database
    pub adapter: NodeTypesAdapter<T>,
    /// container for type specific components
    pub components_builder: CB,
    /// Additional node extensions.
    pub add_ons: AddOns<NodeAdapter<T, CB::Components>, AO>,
}

impl<T, CB> NodeBuilderWithComponents<T, CB, ()>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
{
    /// Advances the state of the node builder to the next state where all customizable
    /// [`NodeAddOns`] types are configured.
    pub fn with_add_ons<AO>(self, add_ons: AO) -> NodeBuilderWithComponents<T, CB, AO>
    where
        AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
    {
        let Self { config, adapter, components_builder, .. } = self;

        NodeBuilderWithComponents {
            config,
            adapter,
            components_builder,
            add_ons: AddOns { hooks: NodeHooks::default(), exexs: Vec::new(), add_ons },
        }
    }
}

impl<T, CB, AO> NodeBuilderWithComponents<T, CB, AO>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: NodeAddOns<NodeAdapter<T, CB::Components>>,
{
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
        F: FnOnce(FullNode<NodeAdapter<T, CB::Components>, AO>) -> eyre::Result<()>
            + Send
            + 'static,
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

    /// Modifies the addons with the given closure.
    pub fn map_add_ons<F>(mut self, f: F) -> Self
    where
        F: FnOnce(AO) -> AO,
    {
        self.add_ons.add_ons = f(self.add_ons.add_ons);
        self
    }
}

/// A builder that includes RPC hooks storage for types that use `RethRpcAddOns`.
/// This type is created when `on_rpc_started` or `extend_rpc_modules` is called
/// on a `NodeBuilderWithComponents` that has `RethRpcAddOns`.
pub struct NodeBuilderWithRpcHooks<T, CB, AO>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: RethRpcAddOns<NodeAdapter<T, CB::Components>> + 'static,
{
    /// The base builder
    pub(crate) inner: NodeBuilderWithComponents<T, CB, AO>,
    /// RPC hooks storage with proper typing from `RethRpcAddOns::EthApi`
    pub(crate) rpc_hooks: RpcHooks<NodeAdapter<T, CB::Components>, AO::EthApi>,
}

impl<T, CB, AO> NodeBuilderWithComponents<T, CB, AO>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: RethRpcAddOns<NodeAdapter<T, CB::Components>> + 'static,
{
    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(self, hook: F) -> NodeBuilderWithRpcHooks<T, CB, AO>
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        let rpc_hooks = RpcHooks { on_rpc_started: Some(Box::new(hook)), ..Default::default() };
        NodeBuilderWithRpcHooks { inner: self, rpc_hooks }
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(self, hook: F) -> NodeBuilderWithRpcHooks<T, CB, AO>
    where
        F: FnOnce(RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        let rpc_hooks = RpcHooks { extend_rpc_modules: Some(Box::new(hook)), ..Default::default() };
        NodeBuilderWithRpcHooks { inner: self, rpc_hooks }
    }

    /// Launches the node with the given launcher.
    pub async fn launch_with<L>(self, launcher: L) -> eyre::Result<L::Node>
    where
        L: LaunchNode<Self>,
    {
        launcher.launch_node(self).await
    }
}

impl<T, CB, AO> NodeBuilderWithRpcHooks<T, CB, AO>
where
    T: FullNodeTypes,
    CB: NodeComponentsBuilder<T>,
    AO: RethRpcAddOns<NodeAdapter<T, CB::Components>> + 'static,
{
    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(
                RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>,
                RethRpcServerHandles,
            ) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.rpc_hooks.on_rpc_started = Some(Box::new(hook));
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, NodeAdapter<T, CB::Components>, AO::EthApi>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.rpc_hooks.extend_rpc_modules = Some(Box::new(hook));
        self
    }

    /// Sets the hook that is run once the node's components are initialized.
    pub fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(NodeAdapter<T, CB::Components>) -> eyre::Result<()> + Send + 'static,
    {
        self.inner.add_ons.hooks.set_on_component_initialized(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    pub fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(FullNode<NodeAdapter<T, CB::Components>, AO>) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.inner.add_ons.hooks.set_on_node_started(hook);
        self
    }

    /// Installs an `ExEx` (Execution Extension) in the node.
    pub fn install_exex<F, R, E>(mut self, exex_id: impl Into<String>, exex: F) -> Self
    where
        F: FnOnce(ExExContext<NodeAdapter<T, CB::Components>>) -> R + Send + 'static,
        R: Future<Output = eyre::Result<E>> + Send,
        E: Future<Output = eyre::Result<()>> + Send,
    {
        self.inner.add_ons.exexs.push((exex_id.into(), Box::new(exex)));
        self
    }

    /// Modifies the addons with the given closure.
    pub fn map_add_ons<F>(mut self, f: F) -> Self
    where
        F: FnOnce(AO) -> AO,
    {
        self.inner.add_ons.add_ons = f(self.inner.add_ons.add_ons);
        self
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
        L: LaunchNode<
            NodeBuilderWithComponents<
                T,
                CB,
                AddOnsWithRpcHooks<NodeAdapter<T, CB::Components>, AO>,
            >,
        >,
    {
        // Wrap the addons with the RPC hooks
        let wrapped_addons =
            AddOnsWithRpcHooks { inner: self.inner.add_ons.add_ons, rpc_hooks: self.rpc_hooks };

        // Create adapted hooks that can work with the wrapped addon type
        let adapted_hooks =
            adapt_node_hooks::<NodeAdapter<T, CB::Components>, AO>(self.inner.add_ons.hooks);

        // Create a new builder with the wrapped addons
        let builder_with_wrapped_addons: NodeBuilderWithComponents<
            T,
            CB,
            AddOnsWithRpcHooks<NodeAdapter<T, CB::Components>, AO>,
        > = NodeBuilderWithComponents {
            config: self.inner.config,
            adapter: self.inner.adapter,
            components_builder: self.inner.components_builder,
            add_ons: AddOns {
                hooks: adapted_hooks,
                exexs: self.inner.add_ons.exexs,
                add_ons: wrapped_addons,
            },
        };

        // Launch with the wrapped builder
        launcher.launch_node(builder_with_wrapped_addons).await
    }
}

/// Adapts node hooks from the original addon type to work with the wrapped addon type.
/// This allows preserving node hooks when wrapping addons with RPC hooks.
fn adapt_node_hooks<N, AO>(
    original_hooks: NodeHooks<N, AO>,
) -> NodeHooks<N, AddOnsWithRpcHooks<N, AO>>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N> + 'static,
{
    let mut new_hooks = NodeHooks::new();
    new_hooks.on_component_initialized = original_hooks.on_component_initialized;
    new_hooks.on_node_started = Box::new(NodeStartedHookAdapter {
        inner: original_hooks.on_node_started,
        _marker: std::marker::PhantomData,
    });
    new_hooks
}

/// Adapter that transforms `FullNode<Node, AddOnsWithRpcHooks<N, AO>>` to `FullNode<Node, AO>`
/// for the original hook.
struct NodeStartedHookAdapter<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N> + 'static,
{
    inner: Box<dyn OnNodeStartedHook<N, AO>>,
    _marker: std::marker::PhantomData<(N, AO)>,
}

impl<N, AO> OnNodeStartedHook<N, AddOnsWithRpcHooks<N, AO>> for NodeStartedHookAdapter<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N> + 'static,
{
    fn on_event(self: Box<Self>, node: FullNode<N, AddOnsWithRpcHooks<N, AO>>) -> eyre::Result<()> {
        // Transform the FullNode to use the original addon type
        // Since AddOnsWithRpcHooks has the same Handle type as the inner AO,
        // we can construct a FullNode with the original addon type
        let unwrapped_node = FullNode {
            evm_config: node.evm_config,
            pool: node.pool,
            network: node.network,
            provider: node.provider,
            payload_builder_handle: node.payload_builder_handle,
            task_executor: node.task_executor,
            config: node.config,
            data_dir: node.data_dir,
            add_ons_handle: node.add_ons_handle, // This is AO::Handle which is the same for both
        };

        // Call the original hook with the unwrapped node
        self.inner.on_event(unwrapped_node)
    }
}

/// A generic wrapper that wraps any addon type along with RPC hooks.
/// This is used to pass RPC hooks through the launch process for any addon type
/// that implements `RethRpcAddOns`.
pub struct AddOnsWithRpcHooks<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N>,
{
    /// The inner addon implementation
    pub inner: AO,
    /// RPC hooks to be used during launch
    pub rpc_hooks: RpcHooks<N, AO::EthApi>,
}

impl<N, AO> AddOnsWithRpcHooks<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N>,
{
    /// Get the inner addon
    pub const fn inner(&self) -> &AO {
        &self.inner
    }
}

impl<N, AO> NodeAddOns<N> for AddOnsWithRpcHooks<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N>,
{
    type Handle = AO::Handle;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        // Use the new trait method to pass hooks to the inner addon
        self.inner.launch_add_ons_with_hooks(ctx, self.rpc_hooks).await
    }
}

impl<N, AO> RethRpcAddOns<N> for AddOnsWithRpcHooks<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N>,
{
    type EthApi = AO::EthApi;
}

impl<N, AO> crate::rpc::EngineValidatorAddOn<N> for AddOnsWithRpcHooks<N, AO>
where
    N: FullNodeComponents,
    AO: RethRpcAddOns<N> + crate::rpc::EngineValidatorAddOn<N>,
{
    type ValidatorBuilder = AO::ValidatorBuilder;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        self.inner.engine_validator_builder()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::components::Components;
    use reth_consensus::noop::NoopConsensus;
    use reth_db_api::mock::DatabaseMock;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm::noop::NoopEvmConfig;
    use reth_evm_ethereum::MockEvmConfig;
    use reth_network::EthNetworkPrimitives;
    use reth_network_api::noop::NoopNetwork;
    use reth_node_api::FullNodeTypesAdapter;
    use reth_node_ethereum::EthereumNode;
    use reth_payload_builder::PayloadBuilderHandle;
    use reth_provider::noop::NoopProvider;
    use reth_tasks::TaskManager;
    use reth_transaction_pool::noop::NoopTransactionPool;

    #[test]
    fn test_noop_components() {
        let components = Components::<
            FullNodeTypesAdapter<EthereumNode, DatabaseMock, NoopProvider>,
            NoopNetwork<EthNetworkPrimitives>,
            _,
            NoopEvmConfig<MockEvmConfig>,
            _,
        > {
            transaction_pool: NoopTransactionPool::default(),
            evm_config: NoopEvmConfig::default(),
            consensus: NoopConsensus::default(),
            network: NoopNetwork::default(),
            payload_builder_handle: PayloadBuilderHandle::<EthEngineTypes>::noop(),
        };

        let task_executor = {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let handle = runtime.handle().clone();
            let manager = TaskManager::new(handle);
            manager.executor()
        };

        let node = NodeAdapter { components, task_executor, provider: NoopProvider::default() };

        // test that node implements `FullNodeComponents``
        <NodeAdapter<_, _> as FullNodeComponents>::pool(&node);
    }
}
