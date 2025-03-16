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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U128};
    use alloy_rpc_types::engine::PayloadStatus;
    use reth_chainspec::ChainInfo;
    use reth_engine_primitives::{BeaconEngineMessage, OnForkChoiceUpdated};
    use reth_eth_wire::NewBlock;
    use reth_node_ethereum::EthEngineTypes;
    use reth_primitives::Block;
    use reth_provider::ProviderError;
    use std::{
        sync::Arc,
        task::{Context, Poll},
    };

    #[derive(Clone)]
    struct MockProvider;

    impl BlockNumReader for MockProvider {
        fn chain_info(&self) -> Result<ChainInfo, ProviderError> {
            unimplemented!()
        }

        fn best_block_number(&self) -> Result<u64, ProviderError> {
            Ok(0)
        }

        fn last_block_number(&self) -> Result<u64, ProviderError> {
            Ok(0)
        }

        fn block_number(&self, _hash: B256) -> Result<Option<u64>, ProviderError> {
            Ok(None)
        }
    }

    impl BlockHashReader for MockProvider {
        fn block_hash(&self, _number: u64) -> Result<Option<B256>, ProviderError> {
            Ok(Some(B256::ZERO))
        }

        fn canonical_hashes_range(
            &self,
            _start: u64,
            _end: u64,
        ) -> Result<Vec<B256>, ProviderError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn can_handle_valid_block() {
        let consensus = Arc::new(ParliaConsensus::new(MockProvider));

        // setup a mock engine
        let (to_engine, mut from_engine) = mpsc::unbounded_channel();
        let engine_handle: BeaconConsensusEngineHandle<EthEngineTypes> =
            BeaconConsensusEngineHandle::new(to_engine);

        // handle the EngineMessages
        tokio::spawn(Box::pin(async move {
            while let Some(message) = from_engine.recv().await {
                match message {
                    BeaconEngineMessage::NewPayload { payload, tx } => {
                        tx.send(Ok(PayloadStatus::new(PayloadStatusEnum::Valid, None))).unwrap();
                    }
                    BeaconEngineMessage::ForkchoiceUpdated {
                        state,
                        payload_attrs,
                        version,
                        tx,
                    } => {
                        tx.send(Ok(OnForkChoiceUpdated::valid(PayloadStatus::new(
                            PayloadStatusEnum::Valid,
                            None,
                        ))))
                        .unwrap();
                    }
                    _ => {}
                }
            }
        }));

        // setup the import service
        let (service, mut handle) = ImportService::new(consensus.clone(), engine_handle);
        tokio::spawn(Box::pin(async move {
            service.await.unwrap();
        }));

        // create an empty block
        let block: reth_primitives::Block = Block::default();
        let new_block = NewBlock { block: block.clone(), td: U128::ZERO };
        let block_msg =
            NewBlockMessage { hash: block.header.hash_slow(), block: Arc::new(new_block) };

        // send the block to the service
        handle.send_block(block_msg.clone(), PeerId::random()).unwrap();

        // poll the service until the block is processed
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        loop {
            match handle.poll_outcome(&mut cx) {
                Poll::Ready(outcome) => {
                    if let Some(outcome) = outcome {
                        println!("Outcome: {:?}", outcome);
                        assert!(matches!(
                            outcome,
                            BlockImportOutcome {
                                peer: _,
                                result: Ok(BlockValidation::ValidBlock { .. })
                            }
                        ));
                        break;
                    }
                }
                Poll::Pending => tokio::task::yield_now().await,
            }
        }
    }
}
