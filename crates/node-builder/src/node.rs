use reth_db::database::Database;
use reth_node_api::{evm::EvmConfig, primitives::NodePrimitives, EngineTypes};
use reth_node_core::cli::components::FullProvider;
use std::marker::PhantomData;

/// The type that configures the entire node.
pub trait NodeTypes: Send + Sync + 'static {
    /// The node's primitive types.
    type Primitives: NodePrimitives;
    /// The node's engine types.
    type Engine: EngineTypes;
    /// The node's evm configuration.
    type Evm: EvmConfig;
}

/// A helper type that also provides access to the builtin provider type of the node.
// TODO naming
pub trait FullNodeTypes: NodeTypes + 'static {
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
    type Evm = Types::Evm;
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
pub struct FullNode<Node> {
    node: Node,
    // TODO add rpc handlers
}
