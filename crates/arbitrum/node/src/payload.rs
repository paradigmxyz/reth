use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_node_api::{FullNodeTypes, NodeTypes, PrimitivesTy, TxTy};
use reth_node_builder::{components::PayloadBuilderBuilder, BuilderContext, PayloadBuilderConfig, PayloadTypes};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use reth_arbitrum_payload::builder::ArbPayloadBuilder;
use reth_arbitrum_evm::ArbEvmConfig;
use eyre::Result;

#[derive(Clone, Default, Debug)]
pub struct ArbPayloadBuilderBuilder;

impl<Types, Node, Pool> PayloadBuilderBuilder<Node, Pool, ArbEvmConfig<Types::ChainSpec, PrimitivesTy<Types>>>
    for ArbPayloadBuilderBuilder
where
    Types: NodeTypes<ChainSpec: EthereumHardforks, Primitives = PrimitivesTy<Types>>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>> + Unpin + 'static,
    Types::Payload: PayloadTypes,
{
    type PayloadBuilder = ArbPayloadBuilder<
        Pool,
        Node::Provider,
        ArbEvmConfig<Types::ChainSpec, PrimitivesTy<Types>>,
        PrimitivesTy<Types>,
        <Types::Payload as PayloadTypes>::PayloadBuilderAttributes
    >;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: ArbEvmConfig<Types::ChainSpec, PrimitivesTy<Types>>,
    ) -> Result<Self::PayloadBuilder> {
        Ok(ArbPayloadBuilder::new(pool, ctx.provider().clone(), evm_config))
    }
}
