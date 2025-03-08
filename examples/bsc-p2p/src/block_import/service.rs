use alloy_rpc_types::engine::PayloadStatusEnum;
use reth_engine_primitives::{BeaconConsensusEngineHandle, EngineTypes};
use reth_network::{
    import::{BlockImportError, BlockImportOutcome, BlockValidation},
    message::NewBlockMessage,
};
use reth_network_api::PeerId;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives::NodePrimitives;
use reth_primitives_traits::Block;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::error;

/// The block type for a given engine
type BscBlock<T> =
    <<<T as PayloadTypes>::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block;

/// Network message containing a new block
pub(crate) type BlockMsg<T> = NewBlockMessage<BscBlock<T>>;

/// Import outcome for a block
pub(crate) type Outcome<T> = BlockImportOutcome<BscBlock<T>>;

/// Channel message type for incoming blocks
type IncomingBlock<T> = (BlockMsg<T>, PeerId);

pub(crate) struct ImportHandle<T: EngineTypes> {
    /// Send the new block to the service
    to_import: UnboundedSender<IncomingBlock<T>>,
    /// Receive the outcome of the import
    pub(crate) import_outcome: UnboundedReceiver<Outcome<T>>,
}

impl<T: EngineTypes> ImportHandle<T> {
    /// Create a new handle and return the service channels
    pub fn new() -> (Self, UnboundedReceiver<IncomingBlock<T>>, UnboundedSender<Outcome<T>>) {
        let (to_import, from_network) = mpsc::unbounded_channel();
        let (to_network, import_outcome) = mpsc::unbounded_channel();

        (Self { to_import, import_outcome }, from_network, to_network)
    }

    /// Sends the block to import to the service
    pub fn send_block(
        &self,
        block: BlockMsg<T>,
        peer_id: PeerId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.to_import.send((block, peer_id))?;
        Ok(())
    }

    /// Poll for the next import outcome
    pub fn poll_outcome(&mut self, cx: &mut Context<'_>) -> Poll<Option<Outcome<T>>> {
        self.import_outcome.poll_recv(cx)
    }
}

pub(crate) struct ImportService<T: EngineTypes> {
    /// The handle to communicate with the engine service
    engine: BeaconConsensusEngineHandle<T>,
    /// Receive the new block from the network
    from_network: UnboundedReceiver<IncomingBlock<T>>,
    /// Send the outcome of the import to the network
    to_network: UnboundedSender<Outcome<T>>,
}

impl<T: EngineTypes> ImportService<T> {
    /// Create a new block import service
    pub(crate) fn new(engine: BeaconConsensusEngineHandle<T>) -> (Self, ImportHandle<T>) {
        let (handle, from_network, to_network) = ImportHandle::new();
        (Self { engine, from_network, to_network }, handle)
    }
}

impl<T: EngineTypes> Future for ImportService<T> {
    type Output = Result<(), Box<dyn std::error::Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let (block, peer_id) = match ready!(self.from_network.poll_recv(cx)) {
                Some(msg) => msg,
                None => {
                    error!("BSC block import service channel closed, service should terminate");
                    return Poll::Ready(Ok(()));
                }
            };
            // TODO: validate header
            let sealed_block = block.block.block.clone().seal();
            let payload = T::block_to_payload(sealed_block);
            let engine = self.engine.clone();
            let to_network = self.to_network.clone();

            tokio::spawn(async move {
                let payload_result = engine.new_payload(payload).await;

                let outcome = match payload_result {
                    Ok(payload_status) => match payload_status.status {
                        PayloadStatusEnum::Valid => Outcome::<T> {
                            peer: peer_id,
                            result: Ok(BlockValidation::ValidBlock { block }),
                        },
                        PayloadStatusEnum::Invalid { validation_error } => Outcome::<T> {
                            peer: peer_id,
                            result: Err(BlockImportError::InvalidPayload(validation_error)),
                        },
                        _ => return,
                    },
                    Err(err) => {
                        Outcome::<T> { peer: peer_id, result: Err(BlockImportError::Engine(err)) }
                    }
                };

                // TODO: FCU handling and send to engine

                let _ = to_network.send(outcome);
            });
        }
    }
}
