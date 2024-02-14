//! Network component for the node builder.

use crate::{node::FullNodeTypes, BuilderContext};
use reth_network::NetworkHandle;
use reth_transaction_pool::TransactionPool;

/// A type that knows how to build the network implementation.
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool> {
    /// Launches the network implementation and returns the handle to it.
    fn build_network(self, ctx: &BuilderContext<Node>, pool: Pool) -> eyre::Result<NetworkHandle>;
}

impl<Node, F, Pool> NetworkBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: FnOnce(&BuilderContext<Node>, Pool) -> eyre::Result<NetworkHandle>,
{
    fn build_network(self, ctx: &BuilderContext<Node>, pool: Pool) -> eyre::Result<NetworkHandle> {
        self(ctx, pool)
    }
}
