use reth_network::{
    config::NetworkMode, EthNetworkPrimitives, NetworkConfig, NetworkManager, PeersInfo,
};
use reth_node_api::TxTy;
use reth_node_builder::{components::NetworkBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypes;
use reth_primitives::{EthPrimitives, PooledTransaction};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_tracing::tracing::info;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// The network builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollNetworkBuilder;

impl<Node, Pool> NetworkBuilder<Node, Pool> for ScrollNetworkBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = EthPrimitives>>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<Consensus = TxTy<Node::Types>, Pooled = PooledTransaction>,
        > + Unpin
        + 'static,
{
    type Primitives = EthNetworkPrimitives;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<reth_network::NetworkHandle> {
        // set the network mode to work.
        let config = ctx.network_config()?;
        let config = NetworkConfig { network_mode: NetworkMode::Work, ..config };

        let network = NetworkManager::builder(config).await?;
        let handle = ctx.start_network(network, pool);
        info!(target: "reth::cli", enode=%handle.local_node_record(), "P2P networking initialized");
        Ok(handle)
    }
}
