use super::handle::ImportHandle;
use crate::block_import::parlia::{ParliaConsensus, ParliaConsensusErr};
use alloy_rpc_types::engine::{ForkchoiceState, PayloadStatusEnum};
use futures::{future::Either, stream::FuturesUnordered, StreamExt};
use reth_engine_primitives::{BeaconConsensusEngineHandle, EngineTypes};
use reth_network::{
    import::{BlockImportError, BlockImportOutcome, BlockValidation},
    message::NewBlockMessage,
};
use reth_network_api::PeerId;
use reth_payload_primitives::{BuiltPayload, EngineApiMessageVersion, PayloadTypes};
use reth_primitives::NodePrimitives;
use reth_primitives_traits::{AlloyBlockHeader, Block};
use reth_provider::{BlockHashReader, BlockNumReader};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
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

/// Future that processes a block import and returns its outcome
type PayloadFut<T> = Pin<Box<dyn Future<Output = Outcome<T>> + Send + Sync>>;

/// Future that processes a forkchoice update and returns its outcome
type FcuFut<T> = Pin<Box<dyn Future<Output = Outcome<T>> + Send + Sync>>;

/// Channel message type for incoming blocks
pub(crate) type IncomingBlock<T> = (BlockMsg<T>, PeerId);

/// A service that handles bidirectional block import communication with the network.
/// It receives new blocks from the network via `from_network` channel and sends back
/// import outcomes via `to_network` channel.
pub struct ImportService<Provider, T>
where
    Provider: BlockNumReader + Clone,
    T: EngineTypes,
{
    /// The handle to communicate with the engine service
    engine: BeaconConsensusEngineHandle<T>,
    /// The consensus implementation
    consensus: Arc<ParliaConsensus<Provider>>,
    /// Receive the new block from the network
    from_network: UnboundedReceiver<IncomingBlock<T>>,
    /// Send the outcome of the import to the network
    to_network: UnboundedSender<Outcome<T>>,
    /// Pending block imports.
    pending_imports: FuturesUnordered<Either<PayloadFut<T>, FcuFut<T>>>,
}

impl<Provider, T> ImportService<Provider, T>
where
    Provider: BlockNumReader + Clone + 'static,
    T: EngineTypes,
{
    /// Create a new block import service
    pub fn new(
        consensus: Arc<ParliaConsensus<Provider>>,
        engine: BeaconConsensusEngineHandle<T>,
    ) -> (Self, ImportHandle<T>) {
        let (to_import, from_network) = mpsc::unbounded_channel();
        let (to_network, import_outcome) = mpsc::unbounded_channel();

        (
            Self {
                engine,
                consensus,
                from_network,
                to_network,
                pending_imports: FuturesUnordered::new(),
            },
            ImportHandle::new(to_import, import_outcome),
        )
    }

    /// Process a new payload and return the outcome
    fn new_payload(&self, block: BlockMsg<T>, peer_id: PeerId) -> PayloadFut<T> {
        let engine = self.engine.clone();

        Box::pin(async move {
            let sealed_block = block.block.block.clone().seal();
            let payload = T::block_to_payload(sealed_block);

            match engine.new_payload(payload).await {
                Ok(payload_status) => match payload_status.status {
                    PayloadStatusEnum::Valid => Outcome::<T> {
                        peer: peer_id,
                        result: Ok(BlockValidation::ValidBlock { block }),
                    },
                    PayloadStatusEnum::Invalid { validation_error } => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(validation_error.into())),
                    },
                    PayloadStatusEnum::Syncing => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(
                            PayloadStatusEnum::Syncing.as_str().into(),
                        )),
                    },
                    _ => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other("Unsupported payload status".into())),
                    },
                },
                Err(err) => {
                    Outcome::<T> { peer: peer_id, result: Err(BlockImportError::Other(err.into())) }
                }
            }
        })
    }

    /// Process a forkchoice update and return the outcome
    fn update_fork_choice(&self, block: BlockMsg<T>, peer_id: PeerId) -> FcuFut<T> {
        let engine = self.engine.clone();
        let consensus = self.consensus.clone();
        let sealed_block = block.block.block.clone().seal();
        let hash = sealed_block.hash();
        let number = sealed_block.number();

        Box::pin(async move {
            let head_block_hash = match consensus.canonical_head(hash, number) {
                Ok(hash) => hash,
                Err(ParliaConsensusErr::Provider(e)) => {
                    return Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(e.into())),
                    }
                }
                Err(ParliaConsensusErr::HeadHashNotFound) => {
                    return Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other("Current head hash not found".into())),
                    }
                }
            };

            let state = ForkchoiceState {
                head_block_hash,
                safe_block_hash: head_block_hash,
                finalized_block_hash: head_block_hash,
            };

            match engine.fork_choice_updated(state, None, EngineApiMessageVersion::default()).await
            {
                Ok(response) => match response.payload_status.status {
                    PayloadStatusEnum::Valid => Outcome::<T> {
                        peer: peer_id,
                        result: Ok(BlockValidation::ValidBlock { block }),
                    },
                    PayloadStatusEnum::Invalid { validation_error } => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(validation_error.into())),
                    },
                    PayloadStatusEnum::Syncing => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(
                            PayloadStatusEnum::Syncing.as_str().into(),
                        )),
                    },
                    _ => Outcome::<T> {
                        peer: peer_id,
                        result: Err(BlockImportError::Other(
                            "Unsupported forkchoice payload status".into(),
                        )),
                    },
                },
                Err(err) => {
                    Outcome::<T> { peer: peer_id, result: Err(BlockImportError::Other(err.into())) }
                }
            }
        })
    }

    /// Add a new block import task to the pending imports
    fn on_new_block(&mut self, block: BlockMsg<T>, peer_id: PeerId) {
        let payload_fut = self.new_payload(block.clone(), peer_id);
        self.pending_imports.push(Either::Left(payload_fut));

        let fcu_fut = self.update_fork_choice(block, peer_id);
        self.pending_imports.push(Either::Right(fcu_fut));
    }
}

impl<Provider, T> Future for ImportService<Provider, T>
where
    Provider: BlockNumReader + BlockHashReader + Clone + 'static + Unpin,
    T: EngineTypes,
{
    type Output = Result<(), Box<dyn std::error::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Receive new blocks from network
        while let Poll::Ready(Some((block, peer_id))) = this.from_network.poll_recv(cx) {
            this.on_new_block(block, peer_id);
        }

        // Process completed imports and send outcomes to network
        while let Poll::Ready(Some(outcome)) = this.pending_imports.poll_next_unpin(cx) {
            if let Err(e) = this.to_network.send(outcome) {
                return Poll::Ready(Err(Box::new(e)));
            }
        }

        Poll::Pending
    }
}
