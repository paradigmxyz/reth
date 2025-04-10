use crate::{
    chainspec::CustomChainSpec,
    primitives::{CustomHeader, CustomNodePrimitives},
};
use alloy_consensus::{Block, BlockBody};
use eyre::Result;
use op_alloy_consensus::OpPooledTransaction;
use reth_ethereum::{
    chainspec::{EthChainSpec, Hardforks},
    network::{NetworkConfig, NetworkHandle, NetworkManager, NetworkPrimitives},
    node::api::{FullNodeTypes, NodeTypes, TxTy},
    pool::{PoolTransaction, TransactionPool},
};
use reth_node_builder::{components::NetworkBuilder, BuilderContext};
use reth_op::{OpReceipt, OpTransactionSigned};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CustomNetworkPrimitives;

impl NetworkPrimitives for CustomNetworkPrimitives {
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody<OpTransactionSigned, CustomHeader>;
    type Block = Block<OpTransactionSigned, CustomHeader>;
    type BroadcastedTransaction = OpTransactionSigned;
    type PooledTransaction = OpPooledTransaction;
    type Receipt = OpReceipt;
}

#[derive(Default)]
pub struct CustomNetworkBuilder {}

impl CustomNetworkBuilder {
    fn network_config<Node>(
        &self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<NetworkConfig<<Node as FullNodeTypes>::Provider, CustomNetworkPrimitives>>
    where
        Node: FullNodeTypes<Types: NodeTypes<ChainSpec: Hardforks>>,
    {
        let args = &ctx.config().network;
        let network_builder = ctx
            .network_config_builder()?
            // apply discovery settings
            .apply(|mut builder| {
                let rlpx_socket = (args.addr, args.port).into();
                if args.discovery.disable_discovery {
                    builder = builder.disable_discv4_discovery();
                }
                if !args.discovery.disable_discovery {
                    builder = builder.discovery_v5(
                        args.discovery.discovery_v5_builder(
                            rlpx_socket,
                            ctx.config()
                                .network
                                .resolved_bootnodes()
                                .or_else(|| ctx.chain_spec().bootnodes())
                                .unwrap_or_default(),
                        ),
                    );
                }

                builder
            });

        let network_config = ctx.build_network_config(network_builder);

        Ok(network_config)
    }
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for CustomNetworkBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<ChainSpec = CustomChainSpec, Primitives = CustomNodePrimitives>,
    >,
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = TxTy<Node::Types>,
                Pooled = OpPooledTransaction,
            >,
        > + Unpin
        + 'static,
{
    type Primitives = CustomNetworkPrimitives;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> Result<NetworkHandle<Self::Primitives>> {
        let network_config = self.network_config(ctx)?;
        let network = NetworkManager::builder(network_config).await?;
        let handle = ctx.start_network(network, pool);

        Ok(handle)
    }
}
