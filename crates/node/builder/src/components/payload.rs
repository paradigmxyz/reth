//! Payload service component for the node builder.

use std::future::Future;

use reth_node_api::NodeTypesWithEngine;
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;

use crate::{BuilderContext, FullNodeTypes};

/// A type that knows how to spawn the payload service.
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Spawns the payload service and returns the handle to it.
    ///
    /// The [`BuilderContext`] is provided to allow access to the node's configuration.
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<
        Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>>,
    > + Send;
}

impl<Node, F, Fut, Pool> PayloadServiceBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool) -> Fut + Send,
    Fut: Future<
            Output = eyre::Result<
                PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>,
            >,
        > + Send,
{
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<
        Output = eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>>,
    > + Send {
        self(ctx, pool)
    }
}
