// no use so far
//! Parlia component for the node builder.
use crate::{BuilderContext, FullNodeTypes};
use reth_bsc_consensus::Parlia;
use std::future::Future;

/// Needed for bsc parlia consensus.
pub trait ParliaBuilder<Node: FullNodeTypes>: Send {
    /// Creates the parlia.
    fn build_parlia(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Parlia>> + Send;
}

impl<Node, F, Fut> ParliaBuilder<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Parlia>> + Send,
{
    fn build_parlia(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Parlia>> {
        self(ctx)
    }
}
