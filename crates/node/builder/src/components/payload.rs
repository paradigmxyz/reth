//! Payload service component for the node builder.

use reth_node_api::PayloadBuilderFor;
use reth_transaction_pool::TransactionPool;
use std::future::Future;

use crate::{BuilderContext, FullNodeTypes};

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Payload builder implementation.
    type PayloadBuilder: PayloadBuilderFor<Node::Types> + Unpin + 'static;

    /// Spawns the payload service and returns the handle to it.
    ///
    /// The [`BuilderContext`] is provided to allow access to the node's configuration.
    fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<Self::PayloadBuilder>> + Send;
}

impl<Node, F, Fut, Pool, Builder> PayloadServiceBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Builder>> + Send,
    Builder: PayloadBuilderFor<Node::Types> + Unpin + 'static,
{
    type PayloadBuilder = Builder;

    fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<Self::PayloadBuilder>> + Send {
        self(ctx, pool)
    }
}
