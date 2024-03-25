use crate::{
    components::{ComponentsBuilder, FullNodeComponents},
    provider::FullProvider,
    rpc::{RethRpcServerHandles, RpcRegistry},
};
use reth_db::database::Database;
use reth_network::NetworkHandle;
pub use reth_node_api::NodeTypes;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
    rpc::builder::{auth::AuthServerHandle, RpcServerHandle},
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::ChainSpec;
use reth_provider::ChainSpecProvider;
use reth_tasks::TaskExecutor;
use std::{marker::PhantomData, sync::Arc};

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

/// A helper type that is downstream of the node types and adds stateful components to the node.
pub trait FullNodeTypes: NodeTypes + 'static {
    /// Underlying database type.
    type DB: Database + Clone + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::DB>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider> {
    pub(crate) types: Types,
    _db: PhantomData<DB>,
    _provider: PhantomData<Provider>,
}

impl<Types, DB, Provider> FullNodeTypesAdapter<Types, DB, Provider> {
    /// Create a new adapter from the given node types.
    pub fn new(types: Types) -> Self {
        Self { types, _db: Default::default(), _provider: Default::default() }
    }
}

impl<Types, DB, Provider> NodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    DB: Send + Sync + 'static,
    Provider: Send + Sync + 'static,
{
    type Primitives = Types::Primitives;
    type Engine = Types::Engine;
    type Evm = Types::Evm;

    fn evm_config(&self) -> Self::Evm {
        self.types.evm_config()
    }
}

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider<DB>,
    DB: Database + Clone + 'static,
{
    type DB = DB;
    type Provider = Provider;
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
