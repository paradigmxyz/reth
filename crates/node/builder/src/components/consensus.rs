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

impl<Node, F, Consensus> ConsensusBuilder<Node> for F
where
    Node: FullNodeTypes,
    Consensus:
        FullConsensus<PrimitivesTy<Node::Types>, Error = ConsensusError> + Clone + Unpin + 'static,
    F: AsyncFnOnce(&BuilderContext<Node>) -> eyre::Result<Consensus> + Send,
    for<'a> <F as AsyncFnOnce<(&'a BuilderContext<Node>,)>>::CallOnceFuture: Send,
{
    type Consensus = Consensus;

    fn build_consensus(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Consensus>> + Send {
        self(ctx)
    }
}
