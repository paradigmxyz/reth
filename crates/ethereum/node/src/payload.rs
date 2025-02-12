//! Payload component configuration for the Ethereum node.

use reth_chainspec::ChainSpec;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::ConfigureEvmFor;
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{FullNodeTypes, NodeTypesWithEngine, PrimitivesTy, TxTy};
use reth_node_builder::{
    components::PayloadServiceBuilder, BuilderContext, PayloadBuilderConfig, PayloadTypes,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// A basic ethereum payload service.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

impl EthereumPayloadBuilder {
    /// A helper method initializing [`reth_ethereum_payload_builder::EthereumPayloadBuilder`] with
    /// the given EVM config.
    pub fn build<Types, Node, Evm, Pool>(
        &self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<
        reth_ethereum_payload_builder::EthereumPayloadBuilder<Pool, Node::Provider, Evm>,
    >
    where
        Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
        Node: FullNodeTypes<Types = Types>,
        Evm: ConfigureEvmFor<PrimitivesTy<Types>>,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
            + Unpin
            + 'static,
        Types::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        let conf = ctx.payload_builder_config();
        Ok(reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            evm_config,
            EthereumBuilderConfig::new(conf.extra_data_bytes()).with_gas_limit(conf.gas_limit()),
        ))
    }
}

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
        &self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        self.build(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
    }
}
