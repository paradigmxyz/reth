//! Network component for the node builder.

use std::future::Future;

use reth_network::{NetworkHandle, NetworkPrimitives};
use reth_transaction_pool::TransactionPool;

use crate::{BuilderContext, FullNodeTypes};

/// A type that knows how to build the network implementation.
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// The primitive types to use for the network.
    type Primitives: NetworkPrimitives;

    /// Launches the network implementation and returns the handle to it.
    fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<NetworkHandle<Self::Primitives>>> + Send;
}

impl<Node, P, F, Fut, Pool> NetworkBuilder<Node, Pool> for F
where
    Node: FullNodeTypes,
    P: NetworkPrimitives,
    Pool: TransactionPool,
    F: Fn(&BuilderContext<Node>, Pool) -> Fut + Send,
    Fut: Future<Output = eyre::Result<NetworkHandle<P>>> + Send,
{
    type Primitives = P;

    fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> impl Future<Output = eyre::Result<NetworkHandle<P>>> + Send {
        self(ctx, pool)
    }
}
