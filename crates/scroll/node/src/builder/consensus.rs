use reth_chainspec::EthChainSpec;
use reth_node_builder::{components::ConsensusBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::{NodeTypes, NodeTypesWithEngine};
use reth_primitives_traits::NodePrimitives;
use reth_scroll_consensus::ScrollBeaconConsensus;
use reth_scroll_forks::ScrollHardforks;
use reth_scroll_primitives::ScrollReceipt;

/// The consensus builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for ScrollConsensusBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypesWithEngine<
            ChainSpec: EthChainSpec + ScrollHardforks,
            Primitives: NodePrimitives<Receipt = ScrollReceipt>,
        >,
    >,
{
    type Consensus = ScrollBeaconConsensus<<Node::Types as NodeTypes>::ChainSpec>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(ScrollBeaconConsensus::new(ctx.chain_spec()))
    }
}
