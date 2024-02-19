//! Payload service component for the node builder.

use crate::{node::FullNodeTypes, BuilderContext};
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool> {
    /// Spawns the payload service and returns the handle to it.
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>;
}

impl<Node, F, Pool> PayloadServiceBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: FnOnce(&BuilderContext<Node>, Pool) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>,
{
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        self(ctx, pool)
    }
}
