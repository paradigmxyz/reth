use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use alloy_rpc_types::engine::PayloadStatusEnum;
use futures::FutureExt;
use reth_engine_primitives::{BeaconConsensusEngineHandle, EngineTypes};
use reth_network::{
    import::{BlockImportError, BlockImportOutcome, BlockValidation},
    message::NewBlockMessage,
};
use reth_payload_primitives::BuiltPayload;
use reth_primitives::NodePrimitives;
use reth_primitives_traits::Block;
use tokio::sync::mpsc;
use tracing::error;

pub(crate) struct BscBlockImportHandle<EngineT: EngineTypes> {
    /// Send the new block to the service.
    to_import: mpsc::UnboundedSender<(
        NewBlockMessage<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
        reth_network_peers::PeerId,
    )>,
    /// Receive the outcome of the import.
    import_outcome: mpsc::UnboundedReceiver<
        BlockImportOutcome<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    >,
}

impl<EngineT: EngineTypes> BscBlockImportHandle<EngineT> {
    /// Sends the block to import to the service.
    pub fn send_block(
        &self,
        block: NewBlockMessage<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
        peer_id: reth_network_peers::PeerId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.to_import.send((block, peer_id))?;
        Ok(())
    }

    /// Receives the outcome of the import from the service.
    pub async fn recv_outcome(
        &mut self,
    ) -> Option<
        BlockImportOutcome<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    > {
        self.import_outcome.recv().await
    }
}

pub(crate) struct BscBlockImportService<EngineT: EngineTypes> {
    /// The handle to communicate with the engine service.
    engine_handle: BeaconConsensusEngineHandle<EngineT>,
    /// Receive the new block from the network.
    from_network: mpsc::UnboundedReceiver<(
        NewBlockMessage<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
        reth_network_peers::PeerId,
    )>,
    /// Send the outcome of the import to the network.
    to_network: mpsc::UnboundedSender<
        BlockImportOutcome<
            <<EngineT::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    >,
}

impl<EngineT: EngineTypes> BscBlockImportService<EngineT> {
    pub(crate) fn new(
        engine_handle: BeaconConsensusEngineHandle<EngineT>,
    ) -> (Self, BscBlockImportHandle<EngineT>) {
        let (to_import, from_network) = mpsc::unbounded_channel();
        let (to_network, import_outcome) = mpsc::unbounded_channel();
        (
            Self { engine_handle, from_network, to_network },
            BscBlockImportHandle { to_import, import_outcome },
        )
    }
}

impl<EngineT: EngineTypes> Future for BscBlockImportService<EngineT> {
    type Output = Result<(), Box<dyn std::error::Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.from_network.poll_recv(cx) {
                Poll::Ready(Some((incoming_block, peer_id))) => {
                    // Convert block to payload and send to engine
                    let sealed_block = incoming_block.block.block.clone().seal();
                    let payload = EngineT::block_to_payload(sealed_block);

                    // Clone channels and handles needed in the spawned task
                    let engine_handle = self.engine_handle.clone();
                    let to_network = self.to_network.clone();

                    // Spawn a new task to handle the payload processing
                    tokio::spawn(async move {
                        let payload_result = engine_handle.new_payload(payload).await;

                        // Create outcome based on payload result
                        let outcome = match payload_result {
                            Ok(payload_status) => match payload_status.status {
                                PayloadStatusEnum::Valid => BlockImportOutcome {
                                    peer: peer_id,
                                    result: Ok(BlockValidation::ValidBlock {
                                        block: incoming_block,
                                    }),
                                },
                                PayloadStatusEnum::Invalid { validation_error } => {
                                    BlockImportOutcome {
                                        peer: peer_id,
                                        result: Err(BlockImportError::InvalidPayload(
                                            validation_error,
                                        )),
                                    }
                                }
                                _ => return,
                            },
                            Err(err) => BlockImportOutcome {
                                peer: peer_id,
                                result: Err(BlockImportError::Engine(err)),
                            },
                        };

                        // Send outcome back to network
                        if let Err(err) = to_network.send(outcome) {
                            error!("Failed to send block import outcome: {}", err);
                        }
                    });
                }
                Poll::Ready(None) => {
                    error!("Channel closed, service should terminate");
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}
