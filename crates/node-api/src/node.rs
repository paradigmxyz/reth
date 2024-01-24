use crate::{evm::EvmConfig, primitives::NodePrimitives, provider::FullProvider, EngineTypes};
use reth_db::database::Database;

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
    type Provider: FullProvider;
}
