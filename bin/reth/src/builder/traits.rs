//! Traits for the builder process.

use std::marker::PhantomData;

use crate::cli::components::FullProvider;
use reth_node_api::{evm::EvmConfig, primitives::NodePrimitives, EngineTypes};
use reth_transaction_pool::TransactionPool;

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, Provider> {
    _types: PhantomData<Types>,
    _provider: PhantomData<Provider>,
}

impl<Types, Provider> Default for FullNodeTypesAdapter<Types, Provider> {
    fn default() -> Self {
        Self { _types: Default::default(), _provider: Default::default() }
    }
}

impl<Types, Provider> NodeTypes for FullNodeTypesAdapter<Types, Provider>
where
    Types: NodeTypes,
{
    type Primitives = Types::Primitives;
    type Engine = Types::Engine;
    type Evm = Types::Evm;
}

impl<Types, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider,
{
    type Provider = Provider;
}

/// A type that configures all the customizable components of the node and knows how to build them.
///
/// This type is stateful and is responsible for instantiating the node's components.
#[async_trait::async_trait]
pub trait NodeComponentsBuilder<Node: FullNodeTypes> {
    /// The transaction pool to use.
    type Pool: TransactionPool;

    /// Builds the transaction pool.
    ///
    /// Note: Implementors are responsible spawning any background tasks required by the pool, e,g,
    /// [maintain_transaction_pool](reth_transaction_pool::maintain::maintain_transaction_pool).
    ///
    /// TODO: this needs required arguments
    async fn build_pool(&mut self) -> eyre::Result<Self::Pool>;

    /// Spawns the payload service and returns a handle to it.
    ///
    /// TODO: this needs required arguments
    async fn spawn_payload_service(&mut self) -> eyre::Result<()>;
}

// TODO add generic builder impl for NodeComponentsBuilder
