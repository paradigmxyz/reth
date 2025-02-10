use reth_node_builder::{components::PayloadServiceBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::{NodeTypesWithEngine, TxTy};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::ScrollEngineTypes;
use reth_scroll_payload::ScrollPayloadTransactions;
use reth_scroll_primitives::ScrollPrimitives;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// Payload builder for Scroll.
#[derive(Debug, Clone, Default, Copy)]
pub struct ScrollPayloadBuilder<Txs = ()> {
    /// Returns the current best transactions from the mempool.
    pub best_transactions: Txs,
}

impl<Node, Pool, Txs> PayloadServiceBuilder<Node, Pool> for ScrollPayloadBuilder<Txs>
where
    Node: FullNodeTypes,
    Node::Types: NodeTypesWithEngine<
        Primitives = ScrollPrimitives,
        Engine = ScrollEngineTypes,
        ChainSpec = ScrollChainSpec,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Txs: ScrollPayloadTransactions<Pool::Transaction>,
{
    type PayloadBuilder = reth_scroll_payload::ScrollEmptyPayloadBuilder;

    async fn build_payload_builder(
        &self,
        _ctx: &BuilderContext<Node>,
        _pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        Ok(reth_scroll_payload::ScrollEmptyPayloadBuilder::default())
    }
}
