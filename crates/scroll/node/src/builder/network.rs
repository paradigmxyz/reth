use reth_network::{
    config::NetworkMode, transform::header::HeaderTransform, NetworkConfig, NetworkManager,
    NetworkPrimitives, PeersInfo,
};
use reth_node_api::TxTy;
use reth_node_builder::{components::NetworkBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypes;
use reth_primitives_traits::BlockHeader;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_primitives::{
    ScrollBlock, ScrollBlockBody, ScrollPrimitives, ScrollReceipt, ScrollTransactionSigned,
};
use reth_tracing::tracing::info;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use scroll_alloy_hardforks::ScrollHardforks;
use std::fmt::Debug;

/// The network builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollNetworkBuilder;

impl<Node, Pool> NetworkBuilder<Node, Pool> for ScrollNetworkBuilder
where
    Node:
        FullNodeTypes<Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = ScrollPrimitives>>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = TxTy<Node::Types>,
                Pooled = scroll_alloy_consensus::ScrollPooledTransaction,
            >,
        > + Unpin
        + 'static,
{
    type Primitives = ScrollNetworkPrimitives;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<reth_network::NetworkHandle<Self::Primitives>> {
        // get the header transform.
        let chain_spec = ctx.chain_spec();
        let transform = ScrollHeaderTransform { chain_spec };

        // set the network mode to work.
        let config = ctx.network_config()?;
        let config = NetworkConfig {
            network_mode: NetworkMode::Work,
            header_transform: Box::new(transform),
            ..config
        };

        let network = NetworkManager::builder(config).await?;
        let handle = ctx.start_network(network, pool);
        info!(target: "reth::cli", enode=%handle.local_node_record(), "P2P networking initialized");
        Ok(handle)
    }
}

/// Network primitive types used by Scroll networks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScrollNetworkPrimitives;

impl NetworkPrimitives for ScrollNetworkPrimitives {
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = ScrollBlockBody;
    type Block = ScrollBlock;
    type BroadcastedTransaction = ScrollTransactionSigned;
    type PooledTransaction = scroll_alloy_consensus::ScrollPooledTransaction;
    type Receipt = ScrollReceipt;
}

/// An implementation of a [`HeaderTransform`] for Scroll.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ScrollHeaderTransform<ChainSpec> {
    chain_spec: ChainSpec,
}

impl<ChainSpec: ScrollHardforks + Debug + Send + Sync + 'static> ScrollHeaderTransform<ChainSpec> {
    /// Returns a new instance of the [`ScrollHeaderTransform`] from the provider chain spec.
    pub const fn new(chain_spec: ChainSpec) -> Self {
        Self { chain_spec }
    }

    /// Returns a new [`ScrollHeaderTransform`] as a [`HeaderTransform`] trait object.
    pub fn boxed<H: BlockHeader>(chain_spec: ChainSpec) -> Box<dyn HeaderTransform<H>> {
        Box::new(Self { chain_spec })
    }
}

impl<H: BlockHeader, ChainSpec: ScrollHardforks + Debug + Send + Sync> HeaderTransform<H>
    for ScrollHeaderTransform<ChainSpec>
{
    fn map(&self, mut header: H) -> H {
        if self.chain_spec.is_euclid_v2_active_at_timestamp(header.timestamp()) {
            // clear the extra data field.
            *header.extra_data_mut() = Default::default()
        }
        header
    }
}
