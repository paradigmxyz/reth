//! Pool component for the node builder.
use crate::{node::FullNodeTypes, BuilderContext};
use reth_transaction_pool::TransactionPool;

/// A type that knows how to build the transaction pool.
pub trait PoolBuilder<Node: FullNodeTypes> {
    /// The transaction pool to build.
    type Pool: TransactionPool + Unpin + 'static;

    /// Creates the transaction pool.
    fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool>;
}

impl<Node, F, Pool> PoolBuilder<Node> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
    F: FnOnce(&BuilderContext<Node>) -> eyre::Result<Pool>,
{
    type Pool = Pool;

    fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        self(ctx)
    }
}
