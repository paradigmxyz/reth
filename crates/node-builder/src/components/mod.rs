//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes](crate::node::FullNodeTypes).

use crate::node::FullNodeTypes;
pub use builder::*;
pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network::NetworkHandle;
use reth_payload_builder::PayloadBuilderHandle;
pub use traits::*;

mod builder;
mod network;
mod payload;
mod pool;
mod traits;

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct NodeComponents<Node: FullNodeTypes, Pool> {
    /// The transaction pool of the node.
    pub transaction_pool: Pool,
    /// The network implementation of the node.
    pub network: NetworkHandle,
    /// The handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
}

impl<Node: FullNodeTypes, Pool> NodeComponents<Node, Pool> {
    /// Returns the handle to the payload builder service.
    pub fn payload_builder(&self) -> PayloadBuilderHandle<Node::Engine> {
        self.payload_builder.clone()
    }
}
