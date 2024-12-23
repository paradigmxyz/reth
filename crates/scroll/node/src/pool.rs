use reth_node_builder::{components::PoolBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypes;
use reth_primitives::EthPrimitives;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_transaction_pool::noop::NoopTransactionPool;

/// Pool builder for Scroll.
#[derive(Debug, Default)]
pub struct ScrollPoolBuilder;

impl<Node> PoolBuilder<Node> for ScrollPoolBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = EthPrimitives>>,
{
    type Pool = NoopTransactionPool;

    async fn build_pool(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        Ok(NoopTransactionPool::default())
    }
}
