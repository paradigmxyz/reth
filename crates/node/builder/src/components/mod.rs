//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes](crate::node::FullNodeTypes).

use crate::FullNodeTypes;
pub use builder::*;
pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network::NetworkHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;

mod builder;
mod network;
mod payload;
mod pool;

/// An abstraction over the components of a node, consisting of:
///  - transaction pool
///  - network
///  - payload builder.
pub trait NodeComponents<NodeTypes: FullNodeTypes>: Clone + Send + Sync + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool + Unpin;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the handle to the network
    fn network(&self) -> &NetworkHandle;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<NodeTypes::Engine>;
}

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct Components<Node: FullNodeTypes, Pool> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The network implementation of the node.
    pub network: NetworkHandle,
    /// The handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
}

impl<Node, Pool> NodeComponents<Node> for Components<Node, Pool>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
{
    type Pool = Pool;

    fn pool(&self) -> &Self::Pool {
        &self.transaction_pool
    }

    fn network(&self) -> &NetworkHandle {
        &self.network
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Node::Engine> {
        &self.payload_builder
    }
}

impl<Node, Pool> Clone for Components<Node, Pool>
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
{
    fn clone(&self) -> Self {
        Self {
            transaction_pool: self.transaction_pool.clone(),
            network: self.network.clone(),
            payload_builder: self.payload_builder.clone(),
        }
    }
}
