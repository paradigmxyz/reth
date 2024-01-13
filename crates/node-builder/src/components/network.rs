//! Network component for the node builder.

use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_transaction_pool::TransactionPool;

/// A type that knows how to build the network implementation.
#[async_trait::async_trait]
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    async fn build_network(self) -> eyre::Result<NetworkHandle>;
}
