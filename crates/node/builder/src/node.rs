// re-export the node api types
pub use reth_node_api::{FullNodeTypes, NodeTypes};

use crate::{components::NodeComponentsBuilder, rpc::RethRpcAddOns, NodeAdapter, NodeAddOns};
use reth_node_api::{EngineTypes, FullNodeComponents, PayloadTypes};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::ChainSpecProvider;
use reth_rpc_api::EngineApiClient;
use reth_rpc_builder::{auth::AuthServerHandle, RpcServerHandle};
use reth_tasks::TaskExecutor;
use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// A [`crate::Node`] is a [`NodeTypes`] that comes with preconfigured components.
///
/// This can be used to configure the builder with a preset of components.
pub trait Node<N: FullNodeTypes>: NodeTypes + Clone {
    /// The type that builds the node's components.
    type ComponentsBuilder: NodeComponentsBuilder<N>;

    /// Exposes the customizable node add-on types.
    type AddOns: NodeAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    /// Returns a [`NodeComponentsBuilder`] for the node.
    fn components_builder(&self) -> Self::ComponentsBuilder;

    /// Returns the node add-ons.
    fn add_ons(&self) -> Self::AddOns;
}

/// A [`Node`] type builder
#[derive(Clone, Default, Debug)]
pub struct AnyNode<N = (), C = (), AO = ()>(PhantomData<N>, C, AO);

impl<N, C, AO> AnyNode<N, C, AO> {
    /// Configures the types of the node.
    pub fn types<T>(self) -> AnyNode<T, C, AO> {
        AnyNode(PhantomData, self.1, self.2)
    }

    /// Sets the node components builder.
    pub fn components_builder<T>(self, value: T) -> AnyNode<N, T, AO> {
        AnyNode(PhantomData, value, self.2)
    }

    /// Sets the node add-ons.
    pub fn add_ons<T>(self, value: T) -> AnyNode<N, C, T> {
        AnyNode(PhantomData, self.1, value)
    }
}

impl<N, C, AO> NodeTypes for AnyNode<N, C, AO>
where
    N: FullNodeTypes,
    C: Clone + Debug + Send + Sync + Unpin + 'static,
    AO: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = <N::Types as NodeTypes>::Primitives;

    type ChainSpec = <N::Types as NodeTypes>::ChainSpec;

    type StateCommitment = <N::Types as NodeTypes>::StateCommitment;

    type Storage = <N::Types as NodeTypes>::Storage;

    type Payload = <N::Types as NodeTypes>::Payload;
}

impl<N, C, AO> Node<N> for AnyNode<N, C, AO>
where
    N: FullNodeTypes + Clone,
    C: NodeComponentsBuilder<N> + Clone + Debug + Sync + Unpin + 'static,
    AO: NodeAddOns<NodeAdapter<N, C::Components>> + Clone + Debug + Sync + Unpin + 'static,
{
    type ComponentsBuilder = C;
    type AddOns = AO;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        self.1.clone()
    }

    fn add_ons(&self) -> Self::AddOns {
        self.2.clone()
    }
}

/// The launched node with all components including RPC handlers.
///
/// This can be used to interact with the launched node.
#[derive(Debug)]
pub struct FullNode<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// The evm configuration.
    pub evm_config: Node::Evm,
    /// The node's transaction pool.
    pub pool: Node::Pool,
    /// Handle to the node's network.
    pub network: Node::Network,
    /// Provider to interact with the node's database
    pub provider: Node::Provider,
    /// Handle to the node's payload builder service.
    pub payload_builder_handle: PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>,
    /// Task executor for the node.
    pub task_executor: TaskExecutor,
    /// The initial node config.
    pub config: NodeConfig<<Node::Types as NodeTypes>::ChainSpec>,
    /// The data dir of the node.
    pub data_dir: ChainPath<DataDirPath>,
    /// The handle to launched add-ons
    pub add_ons_handle: AddOns::Handle,
}

impl<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> Clone for FullNode<Node, AddOns> {
    fn clone(&self) -> Self {
        Self {
            evm_config: self.evm_config.clone(),
            pool: self.pool.clone(),
            network: self.network.clone(),
            provider: self.provider.clone(),
            payload_builder_handle: self.payload_builder_handle.clone(),
            task_executor: self.task_executor.clone(),
            config: self.config.clone(),
            data_dir: self.data_dir.clone(),
            add_ons_handle: self.add_ons_handle.clone(),
        }
    }
}

impl<Payload, Node, AddOns> FullNode<Node, AddOns>
where
    Payload: PayloadTypes,
    Node: FullNodeComponents<Types: NodeTypes<Payload = Payload>>,
    AddOns: NodeAddOns<Node>,
{
    /// Returns the chain spec of the node.
    pub fn chain_spec(&self) -> Arc<<Node::Types as NodeTypes>::ChainSpec> {
        self.provider.chain_spec()
    }
}

impl<Payload, Node, AddOns> FullNode<Node, AddOns>
where
    Payload: PayloadTypes,
    Node: FullNodeComponents<Types: NodeTypes<Payload = Payload>>,
    AddOns: RethRpcAddOns<Node>,
{
    /// Returns the [`RpcServerHandle`] to the started rpc server.
    pub const fn rpc_server_handle(&self) -> &RpcServerHandle {
        &self.add_ons_handle.rpc_server_handles.rpc
    }

    /// Returns the [`AuthServerHandle`] to the started authenticated engine API server.
    pub const fn auth_server_handle(&self) -> &AuthServerHandle {
        &self.add_ons_handle.rpc_server_handles.auth
    }
}

impl<Engine, Node, AddOns> FullNode<Node, AddOns>
where
    Engine: EngineTypes,
    Node: FullNodeComponents<Types: NodeTypes<Payload = Engine>>,
    AddOns: RethRpcAddOns<Node>,
{
    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send authenticated http requests to the node's auth server.
    pub fn engine_http_client(&self) -> impl EngineApiClient<Engine> {
        self.auth_server_handle().http_client()
    }

    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send authenticated ws requests to the node's auth server.
    pub async fn engine_ws_client(&self) -> impl EngineApiClient<Engine> {
        self.auth_server_handle().ws_client().await
    }

    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send not authenticated IPC requests to the node's auth server.
    #[cfg(unix)]
    pub async fn engine_ipc_client(&self) -> Option<impl EngineApiClient<Engine>> {
        self.auth_server_handle().ipc_client().await
    }
}

impl<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> Deref for FullNode<Node, AddOns> {
    type Target = AddOns::Handle;

    fn deref(&self) -> &Self::Target {
        &self.add_ons_handle
    }
}

impl<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> DerefMut for FullNode<Node, AddOns> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.add_ons_handle
    }
}
