use reth_node_builder::{components::PoolBuilder, BuilderContext, FullNodeTypes};
use reth_transaction_pool::noop::NoopTransactionPool;

/// Pool builder for Scroll.
#[derive(Debug)]
pub struct ScrollPoolBuilder;

impl<Node> PoolBuilder<Node> for ScrollPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = NoopTransactionPool;

    async fn build_pool(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        Ok(NoopTransactionPool::default())
    }
}
