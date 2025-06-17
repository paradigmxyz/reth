use reth_evm::ConfigureEvm;
use reth_node_api::PrimitivesTy;
use reth_node_builder::{
    components::PayloadBuilderBuilder, BuilderContext, FullNodeTypes, PayloadBuilderConfig,
};
use reth_node_types::{NodeTypes, TxTy};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::ScrollEngineTypes;
use reth_scroll_evm::ScrollNextBlockEnvAttributes;
use reth_scroll_payload::{ScrollBuilderConfig, ScrollPayloadTransactions};
use reth_scroll_primitives::{ScrollPrimitives, ScrollTransactionSigned};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::time::Duration;

/// Payload builder for Scroll.
#[derive(Debug, Clone, Copy)]
pub struct ScrollPayloadBuilderBuilder<Txs = ()> {
    /// Returns the current best transactions from the mempool.
    pub best_transactions: Txs,
    /// The payload building time limit.
    pub payload_building_time_limit: Duration,
}

impl Default for ScrollPayloadBuilderBuilder {
    fn default() -> Self {
        Self {
            best_transactions: (),
            payload_building_time_limit: SCROLL_PAYLOAD_BUILDING_DURATION,
        }
    }
}

const SCROLL_GAS_LIMIT: u64 = 20_000_000;
const SCROLL_PAYLOAD_BUILDING_DURATION: Duration = Duration::from_secs(1);

impl<Txs> ScrollPayloadBuilderBuilder<Txs> {
    /// A helper method to initialize [`reth_scroll_payload::ScrollPayloadBuilder`] with the
    /// given EVM config.
    pub fn build<Node, Evm, Pool>(
        self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<reth_scroll_payload::ScrollPayloadBuilder<Pool, Node::Provider, Evm, Txs>>
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                Payload = ScrollEngineTypes,
                ChainSpec = ScrollChainSpec,
                Primitives = ScrollPrimitives,
            >,
        >,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
            + Unpin
            + 'static,
        Evm: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>>,
        Txs: ScrollPayloadTransactions<Pool::Transaction>,
    {
        let gas_limit = ctx.payload_builder_config().gas_limit().unwrap_or_else (|| {
            tracing::warn!(target: "reth::cli", "Using {SCROLL_GAS_LIMIT} gas limit for ScrollPayloadBuilder. Configure with --builder.gaslimit");
            SCROLL_GAS_LIMIT
        });

        let payload_builder = reth_scroll_payload::ScrollPayloadBuilder::new(
            pool,
            evm_config,
            ctx.provider().clone(),
            ScrollBuilderConfig::new(gas_limit, self.payload_building_time_limit),
        )
        .with_transactions(self.best_transactions);

        Ok(payload_builder)
    }
}

impl<Node, Pool, Txs, Evm> PayloadBuilderBuilder<Node, Pool, Evm>
    for ScrollPayloadBuilderBuilder<Txs>
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            Payload = ScrollEngineTypes,
            ChainSpec = ScrollChainSpec,
            Primitives = ScrollPrimitives,
        >,
    >,
    Evm: ConfigureEvm<
            Primitives = PrimitivesTy<Node::Types>,
            NextBlockEnvCtx = ScrollNextBlockEnvAttributes,
        > + 'static,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>
        + Unpin
        + 'static,
    Txs: ScrollPayloadTransactions<Pool::Transaction>,
{
    type PayloadBuilder = reth_scroll_payload::ScrollPayloadBuilder<Pool, Node::Provider, Evm, Txs>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: Evm,
    ) -> eyre::Result<Self::PayloadBuilder> {
        self.build(evm_config, ctx, pool)
    }
}
