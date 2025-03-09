use reth::{
    api::{FullNodeTypes, NodeTypesWithEngine, PayloadTypes, TxTy},
    builder::{components::PayloadBuilderBuilder, BuilderContext, PayloadBuilderConfig},
    chainspec::ChainSpec,
    transaction_pool::{PoolTransaction, TransactionPool},
};
use reth_basic_payload_builder::BetterPayloadEmitter;
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumPayloadBuilder};
use reth_node_ethereum::{engine::EthPayloadAttributes, EthEvmConfig};
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::EthPrimitives;
use tokio::sync::broadcast;

#[derive(Debug)]
pub struct BetterPayloadEmitterBuilder {
    better_payloads_tx: broadcast::Sender<EthBuiltPayload>,
}

impl BetterPayloadEmitterBuilder {
    pub const fn new(better_payloads_tx: broadcast::Sender<EthBuiltPayload>) -> Self {
        Self { better_payloads_tx }
    }
}

impl<Types, Node, Pool> PayloadBuilderBuilder<Node, Pool> for BetterPayloadEmitterBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Types::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = EthPayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    type PayloadBuilder =
        BetterPayloadEmitter<EthereumPayloadBuilder<Pool, Node::Provider, EthEvmConfig>>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let conf = ctx.payload_builder_config();
        let client = ctx.provider().clone();
        let evm_config = EthEvmConfig::new(ctx.chain_spec());
        let builder_config = EthereumBuilderConfig::new().with_gas_limit(conf.gas_limit());
        let inner = EthereumPayloadBuilder::new(client, pool, evm_config, builder_config);
        let builder = BetterPayloadEmitter::new(self.better_payloads_tx, inner);
        Ok(builder)
    }
}
