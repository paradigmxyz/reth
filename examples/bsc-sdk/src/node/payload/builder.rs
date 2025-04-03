//! Payload component configuration for the Bsc node.

use crate::{chainspec::BscChainSpec, node::evm::config::BscEvmConfig};
use reth::{
    api::{FullNodeTypes, NodeTypes, TxTy},
    builder::{
        components::PayloadBuilderBuilder, BuilderContext, PayloadBuilderConfig, PrimitivesTy,
    },
    payload::{EthBuiltPayload, EthPayloadBuilderAttributes},
    transaction_pool::{PoolTransaction, TransactionPool},
};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_evm::ConfigureEvm;
use reth_node_ethereum::engine::EthPayloadAttributes;
use reth_payload_primitives::PayloadTypes;
use reth_primitives::EthPrimitives;

/// A basic ethereum payload service.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct BscPayloadBuilder;

impl BscPayloadBuilder {
    /// A helper method initializing [`reth_ethereum_payload_builder::EthereumPayloadBuilder`] with
    /// the given EVM config.
    pub fn build<Types, Node, Evm, Pool>(
        self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<super::BscPayloadBuilder<Pool, Node::Provider, Evm>>
    where
        Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>,
        Node: FullNodeTypes<Types = Types>,
        Evm: ConfigureEvm<Primitives = PrimitivesTy<Types>>,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
            + Unpin
            + 'static,
        Types::Payload: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        let conf = ctx.payload_builder_config();
        Ok(super::BscPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            evm_config,
            EthereumBuilderConfig::new().with_gas_limit(conf.gas_limit()),
        ))
    }
}

impl<Types, Node, Pool> PayloadBuilderBuilder<Node, Pool> for BscPayloadBuilder
where
    Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Types::Payload: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = EthPayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    type PayloadBuilder = super::BscPayloadBuilder<Pool, Node::Provider, BscEvmConfig>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        self.build(BscEvmConfig::new(ctx.chain_spec()), ctx, pool)
    }
}
