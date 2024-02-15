//! Pool component for the node builder.
use crate::{node::FullNodeTypes, BuilderContext};
use reth_transaction_pool::TransactionPool;
use std::future::Future;

/// A type that knows how to build the transaction pool.
pub trait PoolBuilder<Node: FullNodeTypes>: Send {
    /// The transaction pool to build.
    type Pool: TransactionPool + Unpin + 'static;

    /// Creates the transaction pool.
    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> + Send;
}

impl<Node, F, Fut, Pool> PoolBuilder<Node> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Pool>> + Send,
{
    type Pool = Pool;

    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> {
        self(ctx)
    }
}
