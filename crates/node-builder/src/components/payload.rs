//! Payload service component for the node builder.

use reth_node_api::node::FullNodeTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;
use crate::components::BuilderContext;

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool> {
    /// Spawns the payload service and returns the handle to it.
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>;
}


impl<Node: FullNodeTypes, F, Pool> PayloadServiceBuilder<Node, Pool> for F
    where
        F: FnOnce(&BuilderContext<Node>, Pool) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>,
        Pool: TransactionPool,
{
    fn spawn_payload_service(self, ctx: &BuilderContext<Node>, pool: Pool) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        self(ctx, pool)
    }
}