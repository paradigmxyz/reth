use crate::pool::ScrollNoopTransactionPool;
use reth_node_builder::{components::PoolBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypes;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_primitives::ScrollPrimitives;

/// Pool builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollPoolBuilder;

impl<Node> PoolBuilder<Node> for ScrollPoolBuilder
where
    Node:
        FullNodeTypes<Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = ScrollPrimitives>>,
{
    type Pool = ScrollNoopTransactionPool;

    async fn build_pool(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        Ok(ScrollNoopTransactionPool)
    }
}
