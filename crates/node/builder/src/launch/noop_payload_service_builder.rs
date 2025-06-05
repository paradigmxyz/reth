use crate::{components::PayloadServiceBuilder, BuilderContext};
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeTypes, NodeTypes};
use reth_payload_builder::{PayloadBuilderHandle, PayloadServiceCommand};
use reth_transaction_pool::TransactionPool;
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

/// A `NoopPayloadServiceBuilder` useful for node implementations that are not implementing
/// validating/sequencing logic.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NoopPayloadServiceBuilder;

impl<Node, Pool, Evm> PayloadServiceBuilder<Node, Pool, Evm> for NoopPayloadServiceBuilder
where
    Node: FullNodeTypes,
    Pool: TransactionPool,
    Evm: ConfigureEvm,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        _pool: Pool,
        _evm_config: Evm,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        let (tx, mut rx) = mpsc::unbounded_channel();

        ctx.task_executor().spawn_critical("payload builder", async move {
            #[allow(clippy::collection_is_never_read)]
            let mut subscriptions = Vec::new();

            while let Some(message) = rx.recv().await {
                match message {
                    PayloadServiceCommand::Subscribe(tx) => {
                        let (events_tx, events_rx) = broadcast::channel(100);
                        // Retain senders to make sure that channels are not getting closed
                        subscriptions.push(events_tx);
                        let _ = tx.send(events_rx);
                    }
                    message => warn!(?message, "Noop payload service received a message"),
                }
            }
        });

        Ok(PayloadBuilderHandle::new(tx))
    }
}
