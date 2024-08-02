// re-export the node api types
pub use reth_node_api::{FullNodeTypes, NodeTypes};

use std::{marker::PhantomData, sync::Arc};

use reth_chainspec::ChainSpec;
use reth_node_api::FullNodeComponents;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
    rpc::api::EngineApiClient,
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::ChainSpecProvider;
use reth_rpc_builder::{auth::AuthServerHandle, RpcServerHandle};
use reth_tasks::TaskExecutor;

use crate::{
    components::NodeComponentsBuilder,
    rpc::{RethRpcServerHandles, RpcRegistry},
    NodeAdapter, NodeAddOns,
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
}

/// A [`Node`] type builder
#[derive(Clone, Default, Debug)]
pub struct AnyNode<N = (), C = (), AO = ()>(PhantomData<(N, AO)>, C);

impl<N, C> AnyNode<N, C> {
    /// Configures the types of the node.
    pub fn types<T>(self) -> AnyNode<T, C> {
        AnyNode::<T, C>(PhantomData::<(T, ())>, self.1)
    }

    /// Sets the node components builder.
    pub const fn components_builder<T>(&self, value: T) -> AnyNode<N, T> {
        AnyNode::<N, T>(PhantomData::<(N, ())>, value)
    }
}

impl<N, C, AO> NodeTypes for AnyNode<N, C, AO>
where
    N: FullNodeTypes,
    C: Send + Sync + Unpin + 'static,
    AO: Send + Sync + Unpin + Clone + 'static,
{
    type Primitives = N::Primitives;

    type Engine = N::Engine;
}

impl<N, C, AO> Node<N> for AnyNode<N, C, AO>
where
    N: FullNodeTypes + Clone,
    C: NodeComponentsBuilder<N> + Clone + Sync + Unpin + 'static,
    AO: NodeAddOns<NodeAdapter<N, C::Components>>,
{
    type ComponentsBuilder = C;
    type AddOns = AO;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        self.1.clone()
    }
}

/// The launched node with all components including RPC handlers.
///
/// This can be used to interact with the launched node.
#[derive(Debug, Clone)]
pub struct FullNode<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// The evm configuration.
    pub evm_config: Node::Evm,
    /// The executor of the node.
    pub block_executor: Node::Executor,
    /// The node's transaction pool.
    pub pool: Node::Pool,
    /// Handle to the node's network.
    pub network: Node::Network,
    /// Provider to interact with the node's database
    pub provider: Node::Provider,
    /// Handle to the node's payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
    /// Task executor for the node.
    pub task_executor: TaskExecutor,
    /// Handles to the node's rpc servers
    pub rpc_server_handles: RethRpcServerHandles,
    /// The configured rpc namespaces
    pub rpc_registry: RpcRegistry<Node, AddOns::EthApi>,
    /// The initial node config.
    pub config: NodeConfig,
    /// The data dir of the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl<Node, AddOns> FullNode<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    /// Returns the [`ChainSpec`] of the node.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.provider.chain_spec()
    }

    /// Returns the [`RpcServerHandle`] to the started rpc server.
    pub const fn rpc_server_handle(&self) -> &RpcServerHandle {
        &self.rpc_server_handles.rpc
    }

    /// Returns the [`AuthServerHandle`] to the started authenticated engine API server.
    pub const fn auth_server_handle(&self) -> &AuthServerHandle {
        &self.rpc_server_handles.auth
    }

    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send authenticated http requests to the node's auth server.
    pub fn engine_http_client(&self) -> impl EngineApiClient<Node::Engine> {
        self.auth_server_handle().http_client()
    }

    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send authenticated ws requests to the node's auth server.
    pub async fn engine_ws_client(&self) -> impl EngineApiClient<Node::Engine> {
        self.auth_server_handle().ws_client().await
    }

    /// Returns the [`EngineApiClient`] interface for the authenticated engine API.
    ///
    /// This will send not authenticated IPC requests to the node's auth server.
    #[cfg(unix)]
    pub async fn engine_ipc_client(&self) -> Option<impl EngineApiClient<Node::Engine>> {
        self.auth_server_handle().ipc_client().await
    }
}
