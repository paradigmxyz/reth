//! Network component for the node builder.

use crate::components::BuilderContext;
use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_transaction_pool::TransactionPool;

/// A type that knows how to build the network implementation.
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Launches the network implementation and returns the handle to it.
    fn build_network(self, ctx: &BuilderContext<Node>, pool: Pool) -> eyre::Result<NetworkHandle>;
}
