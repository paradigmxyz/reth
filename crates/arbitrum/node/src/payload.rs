use reth_chainspec::EthereumHardforks;
use reth_node_api::{FullNodeTypes, NodeTypes, TxTy};
use reth_node_builder::{components::PayloadBuilderBuilder, BuilderContext, PayloadTypes};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use reth_arbitrum_payload::builder::ArbPayloadBuilder;
use reth_arbitrum_evm::ArbEvmConfig;
use eyre::Result;

#[derive(Clone, Default, Debug)]
pub struct ArbPayloadBuilderBuilder;

impl<Types, Node, Pool> PayloadBuilderBuilder<Node, Pool, ArbEvmConfig<Types::ChainSpec, reth_arbitrum_primitives::ArbPrimitives>>
    for ArbPayloadBuilderBuilder
where
    Types: NodeTypes<ChainSpec: EthereumHardforks, Primitives = reth_arbitrum_primitives::ArbPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>> + Unpin + 'static,
    Types::ChainSpec: reth_arbitrum_chainspec::ArbitrumChainSpec,
    Types::Payload: PayloadTypes<
        BuiltPayload = reth_arbitrum_payload::ArbBuiltPayload<reth_arbitrum_primitives::ArbPrimitives>,
        PayloadBuilderAttributes = reth_payload_builder::EthPayloadBuilderAttributes,
        PayloadAttributes = alloy_rpc_types_engine::PayloadAttributes
    >,
{
    type PayloadBuilder = ArbPayloadBuilder<
        Pool,
        Node::Provider,
        ArbEvmConfig<Types::ChainSpec, reth_arbitrum_primitives::ArbPrimitives>,
        reth_arbitrum_primitives::ArbPrimitives,
        <Types::Payload as PayloadTypes>::PayloadBuilderAttributes
    >;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: ArbEvmConfig<Types::ChainSpec, reth_arbitrum_primitives::ArbPrimitives>,
    ) -> Result<Self::PayloadBuilder> {
        Ok(ArbPayloadBuilder::new(pool, ctx.provider().clone(), evm_config))
    }
}
