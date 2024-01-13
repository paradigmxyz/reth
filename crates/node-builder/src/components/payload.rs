//! Payload service component for the node builder.

use reth_node_api::node::FullNodeTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::TransactionPool;

/// A type that knows how to spawn the payload service.
#[async_trait::async_trait]
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Spawns the payload service and returns the handle to it.
    async fn spawn_payload_service(self) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>;
}
