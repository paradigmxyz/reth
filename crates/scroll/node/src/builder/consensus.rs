use reth_consensus::noop::NoopConsensus;
use reth_node_builder::{components::ConsensusBuilder, BuilderContext, FullNodeTypes};

/// The consensus builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScrollConsensusBuilder;

impl<Node: FullNodeTypes> ConsensusBuilder<Node> for ScrollConsensusBuilder {
    type Consensus = NoopConsensus;

    async fn build_consensus(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(NoopConsensus::default())
    }
}
