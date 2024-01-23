//! Type and trait definitions that describe the node's components.

use reth_network::NetworkHandle;
use reth_transaction_pool::TransactionPool;
use crate::node::FullNodeTypes;

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool;

    /// The events type used to create subscriptions.
    // type Events: CanonStateSubscriptions + Clone + 'static;
    // /// The type that is used to spawn tasks.
    // type Tasks: TaskSpawner + Clone + Unpin + 'static;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> Self::Pool;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns the handle to the network
    fn network(&self) -> NetworkHandle;

    fn payload_builder -> PayloadB
}