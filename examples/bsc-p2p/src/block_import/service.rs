use alloy_primitives::{BlockNumber, B256};
use alloy_rpc_types::engine::{ForkchoiceState, PayloadStatusEnum};
use futures::{stream::FuturesUnordered, StreamExt};
use reth_engine_primitives::{BeaconConsensusEngineHandle, EngineTypes};
use reth_network::{
    import::{BlockImportError, BlockImportOutcome, BlockValidation},
    message::NewBlockMessage,
};
use reth_network_api::PeerId;
use reth_payload_primitives::{BuiltPayload, EngineApiMessageVersion, PayloadTypes};
use reth_primitives::NodePrimitives;
use reth_primitives_traits::{AlloyBlockHeader, Block};
use reth_provider::BlockNumReader;
use std::{
    cmp::Ordering,
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
type ImportFuture<T> = Pin<Box<dyn Future<Output = Outcome<T>> + Send + Sync>>;

pub struct ImportHandle<T: EngineTypes> {
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
pub struct ImportService<Provider: BlockNumReader + Clone, T: EngineTypes> {
    /// The handle to communicate with the engine service
    engine: BeaconConsensusEngineHandle<T>,
    /// The provider
    provider: Provider,
    /// Receive the new block from the network
    from_network: UnboundedReceiver<IncomingBlock<T>>,
    /// Send the outcome of the import to the network
    to_network: UnboundedSender<Outcome<T>>,
    /// Pending block imports.
    pending_imports: FuturesUnordered<ImportFuture<T>>,
}

impl<Provider: BlockNumReader + Clone + 'static, T: EngineTypes> ImportService<Provider, T> {
    /// Create a new block import service
    pub fn new(
        provider: Provider,
        engine: BeaconConsensusEngineHandle<T>,
    ) -> (Self, ImportHandle<T>) {
        let (to_import, from_network) = mpsc::unbounded_channel();
        let (to_network, import_outcome) = mpsc::unbounded_channel();

        (
            Self {
                engine,
                provider,
                from_network,
                to_network,
                pending_imports: FuturesUnordered::new(),
            },
            ImportHandle::new(to_import, import_outcome),
        )
    }

    /// Sends a forkchoice update to the engine with the given block hash and number
    async fn send_fork_choice_update(
        engine: &BeaconConsensusEngineHandle<T>,
        provider: &Provider,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> Result<(), BlockImportError> {
        let current_head =
            provider.best_block_number().map_err(|e| BlockImportError::Other(e.into()))?;
        let current_head_hash = provider
            .block_hash(current_head)
            .map_err(|e| BlockImportError::Other(e.into()))?
            .ok_or_else(|| BlockImportError::Other("Current head hash not found".into()))?;

        let head_block_hash =
            canonical_head(block_hash, block_number, current_head, current_head_hash);

        let state = ForkchoiceState {
            head_block_hash,
            safe_block_hash: current_head_hash,
            finalized_block_hash: current_head_hash,
        };

        engine
            .fork_choice_updated(state, None, EngineApiMessageVersion::default())
            .await
            .map_err(|e| BlockImportError::Other(e.into()))
            .and_then(|response| match response.payload_status.status {
                PayloadStatusEnum::Valid => Ok(()),
                PayloadStatusEnum::Invalid { validation_error } => {
                    Err(BlockImportError::Other(validation_error.into()))
                }
                PayloadStatusEnum::Syncing => {
                    Err(BlockImportError::Other(PayloadStatusEnum::Syncing.as_str().into()))
                }
                _ => Err(BlockImportError::Other("Unsupported forkchoice payload status".into())),
            })
    }

    /// Sends a new payload to the engine and waits for the outcome.
    /// If it is valid, proceeds with Forkchoice Update (FCU).
    fn import(&self, block: BlockMsg<T>, peer_id: PeerId) -> ImportFuture<T> {
        let engine = self.engine.clone();
        let provider = self.provider.clone();

        Box::pin(async move {
            let sealed_block = block.block.block.clone().seal();
            let hash = sealed_block.hash();
            let number = sealed_block.number();
            let payload = T::block_to_payload(sealed_block);

            match engine.new_payload(payload).await {
                Ok(payload_status) => match payload_status.status {
                    PayloadStatusEnum::Valid => {
                        // Send forkchoice update if the payload is valid
                        if let Err(err) =
                            Self::send_fork_choice_update(&engine, &provider, hash, number).await
                        {
                            return Outcome::<T> { peer: peer_id, result: Err(err) };
                        }

                        Outcome::<T> {
                            peer: peer_id,
                            result: Ok(BlockValidation::ValidBlock { block }),
                        }
                    }
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

    /// Add a new block import task to the pending imports
    fn on_new_block(&mut self, block: BlockMsg<T>, peer_id: PeerId) {
        self.pending_imports.push(self.import(block, peer_id));
    }
}

impl<Provider: BlockNumReader + Clone + 'static + Unpin, T: EngineTypes> Future
    for ImportService<Provider, T>
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

/// Determines the head block hash according to Parlia consensus rules:
/// 1. Follow the highest block number
/// 2. For same height blocks, pick the one with lower hash
pub(crate) fn canonical_head(
    current_hash: B256,
    current_number: BlockNumber,
    head_number: BlockNumber,
    head_hash: B256,
) -> B256 {
    match current_number.cmp(&head_number) {
        Ordering::Greater => current_hash,
        Ordering::Equal => current_hash.min(head_hash),
        Ordering::Less => head_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, B256};

    #[test]
    fn test_canonical_head() {
        let hash1 = B256::from_slice(&hex!(
            "1111111111111111111111111111111111111111111111111111111111111111"
        ));
        let hash2 = B256::from_slice(&hex!(
            "2222222222222222222222222222222222222222222222222222222222222222"
        ));

        let test_cases = [
            ((hash1, 2, 1, hash2), hash1), // Higher block wins
            ((hash1, 1, 2, hash2), hash2), // Lower block stays
            ((hash1, 1, 1, hash2), hash1), // Same height, lower hash wins
            ((hash2, 1, 1, hash1), hash1), // Same height, lower hash stays
        ];

        for ((curr_hash, curr_num, head_num, head_hash), expected) in test_cases {
            assert_eq!(canonical_head(curr_hash, curr_num, head_num, head_hash), expected);
        }
    }
}
