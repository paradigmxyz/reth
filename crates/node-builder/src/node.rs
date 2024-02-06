use reth_db::database::Database;
use reth_node_api::{evm::EvmConfig, primitives::NodePrimitives, EngineTypes};
use reth_node_core::cli::components::FullProvider;
use std::marker::PhantomData;

/// The type that configures stateless node types, the node's primitive types.
pub trait NodeTypes: Send + Sync + 'static {
    /// The node's primitive types.
    type Primitives: NodePrimitives;
    /// The node's engine types.
    type Engine: EngineTypes;
}

/// A helper type that is downstream of the node types and adds stateful components to the node.
pub trait FullNodeTypes: NodeTypes + 'static {
    /// The node's evm configuration.
    ///
    /// TODO: This might depend on the node's database and provider types.
    type Evm: EvmConfig;
    /// Underlying database type.
    type DB: Database + Clone + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::DB>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider> {
    _types: PhantomData<Types>,
    _db: PhantomData<DB>,
    _provider: PhantomData<Provider>,
}

impl<Types, DB, Provider> Default for FullNodeTypesAdapter<Types, DB, Provider> {
    fn default() -> Self {
        Self { _types: Default::default(), _db: Default::default(), _provider: Default::default() }
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
}

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider<DB>,
    DB: Database + Clone + 'static,
{
    type Evm = Types::Evm;
    type DB = DB;
    type Provider = Provider;
}

/// The launched node with all components including RPC handlers.
#[derive(Debug)]
pub struct FullNode<Node> {
    node: Node,
    // TODO add rpc handlers
}
