use crate::{
    components::{ComponentsBuilder, FullNodeComponents},
    rpc::{RethRpcServerHandles, RpcRegistry},
};
use reth_network::NetworkHandle;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
    rpc::{
        api::EngineApiClient,
        builder::{auth::AuthServerHandle, RpcServerHandle},
    },
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::ChainSpec;
use reth_provider::ChainSpecProvider;
use reth_tasks::TaskExecutor;
use std::sync::Arc;

// re-export the node api types
pub use reth_node_api::{FullNodeTypes, NodeTypes};

/// A [Node] is a [NodeTypes] that comes with preconfigured components.
///
/// This can be used to configure the builder with a preset of components.
pub trait Node<N>: NodeTypes + Clone {
    /// The type that builds the node's pool.
    type PoolBuilder;
    /// The type that builds the node's network.
    type NetworkBuilder;
    /// The type that builds the node's payload service.
    type PayloadBuilder;

    /// Returns the [ComponentsBuilder] for the node.
    fn components(
        self,
    ) -> ComponentsBuilder<N, Self::PoolBuilder, Self::PayloadBuilder, Self::NetworkBuilder>;
}

/// The launched node with all components including RPC handlers.
///
/// This can be used to interact with the launched node.
#[derive(Debug)]
pub struct FullNode<Node: FullNodeComponents> {
    /// The evm configuration.
    pub evm_config: Node::Evm,
    /// The node's transaction pool.
    pub pool: Node::Pool,
    /// Handle to the node's network.
    pub network: NetworkHandle,
    /// Provider to interact with the node's database
    pub provider: Node::Provider,
    /// Handle to the node's payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
    /// Task executor for the node.
    pub task_executor: TaskExecutor,
    /// Handles to the node's rpc servers
    pub rpc_server_handles: RethRpcServerHandles,
    /// The configured rpc namespaces
    pub rpc_registry: RpcRegistry<Node>,
    /// The initial node config.
    pub config: NodeConfig,
    /// The data dir of the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl<Node: FullNodeComponents> FullNode<Node> {
    /// Returns the [ChainSpec] of the node.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.provider.chain_spec()
    }

    /// Returns the [RpcServerHandle] to the started rpc server.
    pub fn rpc_server_handle(&self) -> &RpcServerHandle {
        &self.rpc_server_handles.rpc
    }

    /// Returns the [AuthServerHandle] to the started authenticated engine API server.
    pub fn auth_server_handle(&self) -> &AuthServerHandle {
        &self.rpc_server_handles.auth
    }

    /// Returns the [EngineApiClient] interface for the authenticated engine API.
    ///
    /// This will send authenticated http requests to the node's auth server.
    pub fn engine_http_client(&self) -> impl EngineApiClient<Node::Engine> {
        self.auth_server_handle().http_client()
    }

    /// Returns the [EngineApiClient] interface for the authenticated engine API.
    ///
    /// This will send authenticated ws requests to the node's auth server.
    pub async fn engine_ws_client(&self) -> impl EngineApiClient<Node::Engine> {
        self.auth_server_handle().ws_client().await
    }
}

impl<Node: FullNodeComponents> Clone for FullNode<Node> {
    fn clone(&self) -> Self {
        Self {
            evm_config: self.evm_config.clone(),
            pool: self.pool.clone(),
            network: self.network.clone(),
            provider: self.provider.clone(),
            payload_builder: self.payload_builder.clone(),
            task_executor: self.task_executor.clone(),
            rpc_server_handles: self.rpc_server_handles.clone(),
            rpc_registry: self.rpc_registry.clone(),
            config: self.config.clone(),
            data_dir: self.data_dir.clone(),
        }
    }
}
