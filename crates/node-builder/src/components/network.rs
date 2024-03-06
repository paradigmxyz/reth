//! Network component for the node builder.

use crate::{node::FullNodeTypes, BuilderContext};
use reth_network::NetworkHandle;
use reth_transaction_pool::TransactionPool;
use std::future::Future;

/// A type that knows how to build the network implementation.
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Launches the network implementation and returns the handle to it.
    fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<NetworkHandle>> + Send;
}

impl<Node, F, Fut, Pool> NetworkBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool) -> Fut + Send,
    Fut: Future<Output = eyre::Result<NetworkHandle>> + Send,
{
    fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<NetworkHandle>> + Send {
        self(ctx, pool)
    }
}
