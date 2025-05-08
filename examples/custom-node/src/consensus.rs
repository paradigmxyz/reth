use std::sync::Arc;

use reth_node_builder::{
    components::ConsensusBuilder, BuilderContext, FullNodeTypes, NodePrimitives, NodeTypes,
};
use reth_op::DepositReceipt;
use reth_optimism_consensus::OpBeaconConsensus;
use reth_optimism_forks::OpHardforks;

#[derive(Debug, Default, Clone)]
pub struct CustomConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for CustomConsensusBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec: OpHardforks,
            Primitives: NodePrimitives<Receipt: DepositReceipt>,
        >,
    >,
{
    type Consensus = Arc<OpBeaconConsensus<<Node::Types as NodeTypes>::ChainSpec>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(OpBeaconConsensus::new(ctx.chain_spec())))
    }
}
