use alloy_rpc_types::engine::PayloadStatusEnum;
use futures::{stream::FuturesUnordered, StreamExt};
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
    task::{Context, Poll},
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The block type for a given engine
type BscBlock<T> =
    <<<T as PayloadTypes>::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block;

/// Network message containing a new block
pub(crate) type BlockMsg<T> = NewBlockMessage<BscBlock<T>>;

/// Import outcome for a block
pub(crate) type Outcome<T> = BlockImportOutcome<BscBlock<T>>;

/// Channel message type for incoming blocks
type IncomingBlock<T> = (BlockMsg<T>, PeerId);

/// Future that processes a block import and returns its outcome
type ImportFuture<T> = Pin<Box<dyn Future<Output = Outcome<T>> + Send>>;

pub(crate) struct ImportHandle<T: EngineTypes> {
    /// Send the new block to the service
    to_import: UnboundedSender<IncomingBlock<T>>,
    /// Receive the outcome of the import
    import_outcome: UnboundedReceiver<Outcome<T>>,
}

impl<T: EngineTypes> ImportHandle<T> {
    /// Create a new handle with the provided channels
    pub fn new(
        to_import: UnboundedSender<IncomingBlock<T>>,
        import_outcome: UnboundedReceiver<Outcome<T>>,
    ) -> Self {
        Self { to_import, import_outcome }
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

/// A service that handles bidirectional block import communication with the network.
/// It receives new blocks from the network via `from_network` channel and sends back
/// import outcomes via `to_network` channel.
pub(crate) struct ImportService<T: EngineTypes> {
    /// The handle to communicate with the engine service
    engine: BeaconConsensusEngineHandle<T>,
    /// Receive the new block from the network
    from_network: UnboundedReceiver<IncomingBlock<T>>,
    /// Send the outcome of the import to the network
    to_network: UnboundedSender<Outcome<T>>,
    /// Pending block imports.
    pending_imports: FuturesUnordered<ImportFuture<T>>,
}

impl<T: EngineTypes> ImportService<T> {
    /// Create a new block import service
    pub(crate) fn new(engine: BeaconConsensusEngineHandle<T>) -> (Self, ImportHandle<T>) {
        let (to_import, from_network) = mpsc::unbounded_channel();
        let (to_network, import_outcome) = mpsc::unbounded_channel();

        (
            Self { engine, from_network, to_network, pending_imports: FuturesUnordered::new() },
            ImportHandle::new(to_import, import_outcome),
        )
    }

    /// Add a new block import task to the pending imports
    fn import(&mut self, block: BlockMsg<T>, peer_id: PeerId) {
        let engine = self.engine.clone();

        let fut = async move {
            let sealed_block = block.block.block.clone().seal();
            let payload = T::block_to_payload(sealed_block);
            let payload_result = engine.new_payload(payload).await;
            match payload_result {
                Ok(payload_status) => match payload_status.status {
                    PayloadStatusEnum::Valid => Outcome::<T> {
                        peer: peer_id,
                        result: Ok(BlockValidation::ValidBlock { block }),
                    },
                    PayloadStatusEnum::Invalid { validation_error } => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::InvalidPayload(validation_error)),
                    },
                    _ => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::InvalidPayload(
                            "Unsupported payload status".into(),
                        )),
                    },
                },
                Err(err) => {
                    Outcome::<T> { peer: peer_id, result: Err(BlockImportError::Engine(err)) }
                }
            }
        };

        self.pending_imports.push(Box::pin(fut));
    }
}

impl<T: EngineTypes> Future for ImportService<T> {
    type Output = Result<(), Box<dyn std::error::Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Receive new blocks from network
        while let Poll::Ready(Some((block, peer_id))) = self.from_network.poll_recv(cx) {
            self.import(block, peer_id);
        }

        // Process completed imports and send outcomes to network
        while let Poll::Ready(Some(outcome)) = self.pending_imports.poll_next_unpin(cx) {
            if let Err(e) = self.to_network.send(outcome) {
                return Poll::Ready(Err(Box::new(e)));
            }
        }

        Poll::Pending
    }
}
