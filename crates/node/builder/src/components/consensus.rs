//! Consensus component for the node builder.
use reth_consensus::{ConsensusError, FullConsensus};
use reth_node_api::PrimitivesTy;

use crate::{BuilderContext, FullNodeTypes};
use std::future::Future;

/// A type that knows how to build the consensus implementation.
pub trait ConsensusBuilder<Node: FullNodeTypes>: Send {
    /// The consensus implementation to build.
    type Consensus: FullConsensus<PrimitivesTy<Node::Types>, Error = ConsensusError>
        + Clone
        + Unpin
        + 'static;

    /// Creates the consensus implementation.
    fn build_consensus(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Consensus>> + Send;
}

impl<Node, F, Fut, Consensus> ConsensusBuilder<Node> for F
where
    Node: FullNodeTypes,
    Consensus:
        FullConsensus<PrimitivesTy<Node::Types>, Error = ConsensusError> + Clone + Unpin + 'static,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Consensus>> + Send,
{
    type Consensus = Consensus;

    fn build_consensus(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Consensus>> {
        self(ctx)
    }
}
