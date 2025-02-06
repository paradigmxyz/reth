//! Payload component configuration for the Ethereum node.

use reth_chainspec::ChainSpec;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{FullNodeTypes, NodeTypesWithEngine, TxTy};
use reth_node_builder::{
    components::PayloadServiceBuilder, BuilderContext, PayloadBuilderConfig, PayloadTypes,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// A basic ethereum payload service.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder
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
        reth_ethereum_payload_builder::EthereumPayloadBuilder<Pool, Node::Provider, EthEvmConfig>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let conf = ctx.payload_builder_config();

        Ok(reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            EthEvmConfig::new(ctx.chain_spec()),
            EthereumBuilderConfig::new(conf.extra_data_bytes()).with_gas_limit(conf.gas_limit()),
        ))
    }
}
