use crate::{
    components::FullNodeComponents,
    rpc::{RethRpcServerHandles, RpcRegistry},
};
use reth_db::database::Database;
use reth_network::NetworkHandle;
use reth_node_api::{evm::ConfigureEvm, primitives::NodePrimitives, EngineTypes};
use reth_node_core::{
    cli::components::FullProvider,
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;
use std::marker::PhantomData;

/// The type that configures stateless node types, the node's primitive types.
pub trait NodeTypes: Send + Sync + 'static {
    /// The node's primitive types.
    type Primitives: NodePrimitives;
    /// The node's engine types.
    type Engine: EngineTypes;
    /// The node's evm configuration.
    type Evm: ConfigureEvm;

    /// Returns the node's evm config.
    fn evm_config(&self) -> Self::Evm;
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
#[derive(Debug)]
pub struct FullNode<Node: FullNodeComponents> {
    pub(crate) evm_config: Node::Evm,
    pub(crate) pool: Node::Pool,
    pub(crate) network: NetworkHandle,
    pub(crate) provider: Node::Provider,
    pub(crate) payload_builder: PayloadBuilderHandle<Node::Engine>,
    pub(crate) executor: TaskExecutor,
    pub(crate) rpc_server_handles: RethRpcServerHandles,
    pub(crate) rpc_registry: RpcRegistry<Node>,
    /// The initial node config.
    pub(crate) config: NodeConfig,
    /// The data dir of the node.
    pub(crate) data_dir: ChainPath<DataDirPath>,
}

impl<Node: FullNodeComponents> Clone for FullNode<Node> {
    fn clone(&self) -> Self {
        Self {
            evm_config: self.evm_config.clone(),
            pool: self.pool.clone(),
            network: self.network.clone(),
            provider: self.provider.clone(),
            payload_builder: self.payload_builder.clone(),
            executor: self.executor.clone(),
            rpc_server_handles: self.rpc_server_handles.clone(),
            rpc_registry: self.rpc_registry.clone(),
            config: self.config.clone(),
            data_dir: self.data_dir.clone(),
        }
    }
}
