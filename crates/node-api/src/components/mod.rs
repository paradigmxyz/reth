//! Type and trait definitions that describe the node's components.

use crate::node::FullNodeTypes;
use reth_transaction_pool::TransactionPool;

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
