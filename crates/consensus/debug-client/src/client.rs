use alloy_consensus::{BlockHeader, Sealable};
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadStatusEnum;
use reth_node_api::{
    BuiltPayload, ConsensusEngineHandle, EngineApiMessageVersion, ExecutionPayload, NodePrimitives,
    PayloadTypes,
};
use reth_primitives_traits::{Block, SealedBlock};
use reth_tracing::tracing::{debug, warn};
use ringbuffer::{AllocRingBuffer, RingBuffer};
use std::future::Future;
use tokio::sync::mpsc;

/// Supplies consensus client with new blocks sent in `tx` and a callback to find specific blocks
/// by number to fetch past finalized and safe blocks.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockProvider: Send + Sync + 'static {
    /// The block type.
    type Block: Block;

    /// Runs a block provider to send new blocks to the given sender.
    ///
    /// Note: This is expected to be spawned in a separate task, and as such it should ignore
    /// errors.
    fn subscribe_blocks(&self, tx: mpsc::Sender<Self::Block>) -> impl Future<Output = ()> + Send;

    /// Get a past block by number.
    fn get_block(
        &self,
        block_number: u64,
    ) -> impl Future<Output = eyre::Result<Self::Block>> + Send;

    /// Get previous block hash using previous block hash buffer. If it isn't available (buffer
    /// started more recently than `offset`), fetch it using `get_block`.
    fn get_or_fetch_previous_block(
        &self,
        previous_block_hashes: &AllocRingBuffer<B256>,
        current_block_number: u64,
        offset: usize,
    ) -> impl Future<Output = eyre::Result<B256>> + Send {
        async move {
            if let Some(hash) = get_hash_at_offset(previous_block_hashes, offset) {
                return Ok(hash);
            }

            // Return zero hash if the chain isn't long enough to have the block at the offset.
            let previous_block_number = match current_block_number.checked_sub(offset as u64) {
                Some(number) => number,
                None => return Ok(B256::default()),
            };
            let block = self.get_block(previous_block_number).await?;
            Ok(block.header().hash_slow())
        }
    }
}

/// Debug consensus client that sends FCUs and new payloads using recent blocks from an external
/// provider like Etherscan or an RPC endpoint.
#[derive(Debug)]
pub struct DebugConsensusClient<P: BlockProvider, T: PayloadTypes> {
    /// Handle to execution client.
    engine_handle: ConsensusEngineHandle<T>,
    /// Provider to get consensus blocks from.
    block_provider: P,
}

impl<P: BlockProvider, T: PayloadTypes> DebugConsensusClient<P, T> {
    /// Create a new debug consensus client with the given handle to execution
    /// client and block provider.
    pub const fn new(engine_handle: ConsensusEngineHandle<T>, block_provider: P) -> Self {
        Self { engine_handle, block_provider }
    }
}

impl<P, T> DebugConsensusClient<P, T>
where
    P: BlockProvider + Clone,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = P::Block>>>,
{
    /// Spawn the client to start sending FCUs and new payloads by periodically fetching recent
    /// blocks.
    pub async fn run(self) {
        let mut previous_block_hashes = AllocRingBuffer::new(65);
        let mut block_stream = {
            let (tx, rx) = mpsc::channel::<P::Block>(64);
            let block_provider = self.block_provider.clone();
            tokio::spawn(async move {
                block_provider.subscribe_blocks(tx).await;
            });
            rx
        };

        while let Some(block) = block_stream.recv().await {
            let block_number = block.header().number();

            // Submit the block, backfilling missing parents if the engine returns Syncing.
            if let Err(err) =
                self.submit_block_with_backfill(block, &mut previous_block_hashes).await
            {
                warn!(target: "consensus::debug-client", %block_number, %err, "failed to submit block");
                continue;
            }

            let block_hash = match previous_block_hashes.back() {
                Some(hash) => *hash,
                None => continue,
            };

            // Load previous block hashes. We're using (head - 32) and (head - 64) as the safe and
            // finalized block hashes.
            let safe_block_hash = self.block_provider.get_or_fetch_previous_block(
                &previous_block_hashes,
                block_number,
                32,
            );
            let finalized_block_hash = self.block_provider.get_or_fetch_previous_block(
                &previous_block_hashes,
                block_number,
                64,
            );
            let (safe_block_hash, finalized_block_hash) =
                tokio::join!(safe_block_hash, finalized_block_hash);
            let (safe_block_hash, finalized_block_hash) = match (
                safe_block_hash,
                finalized_block_hash,
            ) {
                (Ok(safe_block_hash), Ok(finalized_block_hash)) => {
                    (safe_block_hash, finalized_block_hash)
                }
                (safe_block_hash, finalized_block_hash) => {
                    warn!(target: "consensus::debug-client", ?safe_block_hash, ?finalized_block_hash, "failed to fetch safe or finalized hash from etherscan");
                    continue;
                }
            };
            let state = alloy_rpc_types_engine::ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash,
                finalized_block_hash,
            };
            let _ = self
                .engine_handle
                .fork_choice_updated(state, None, EngineApiMessageVersion::V3)
                .await;
        }
    }

    /// Submits a block via `newPayload`. If the engine returns `Syncing` (parent state missing),
    /// fetches and submits missing ancestor blocks oldest-to-newest, then retries the original
    /// block.
    ///
    /// Returns `Ok(())` only if the block was ultimately accepted (`Valid`/`Accepted`).
    /// Returns `Err` if the block could not be accepted, so the caller can skip FCU.
    async fn submit_block_with_backfill(
        &self,
        block: P::Block,
        previous_block_hashes: &mut AllocRingBuffer<B256>,
    ) -> eyre::Result<()> {
        let block_number = block.header().number();
        let payload = T::block_to_payload(SealedBlock::new_unhashed(block));
        let block_hash = payload.block_hash();

        let status = self
            .engine_handle
            .new_payload(payload.clone())
            .await
            .map_err(|e| eyre::eyre!("newPayload failed: {e}"))?;

        match status.status {
            PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                previous_block_hashes.enqueue(block_hash);
                Ok(())
            }
            PayloadStatusEnum::Syncing => {
                debug!(
                    target: "consensus::debug-client",
                    %block_number,
                    %block_hash,
                    "newPayload returned Syncing, backfilling missing parents"
                );

                if block_number == 0 {
                    return Err(eyre::eyre!("genesis block returned Syncing"));
                }

                // Walk backwards from this block's parent to find and submit missing ancestors.
                // Cap the backfill depth to avoid unbounded fetching.
                const MAX_BACKFILL_DEPTH: u64 = 64;
                let start = block_number.saturating_sub(1);
                let end = block_number.saturating_sub(MAX_BACKFILL_DEPTH);

                let mut ancestors = Vec::new();
                for num in (end..=start).rev() {
                    match self.block_provider.get_block(num).await {
                        Ok(ancestor) => ancestors.push(ancestor),
                        Err(err) => {
                            warn!(
                                target: "consensus::debug-client",
                                %num, %err, "failed to fetch ancestor block for backfill"
                            );
                            break;
                        }
                    }

                    // Submit each ancestor immediately to probe whether the engine knows it.
                    // This avoids fetching all the way back when only a few blocks are missing.
                    let ancestor_payload = T::block_to_payload(SealedBlock::new_unhashed(
                        ancestors.last().unwrap().clone(),
                    ));
                    let ancestor_status = self
                        .engine_handle
                        .new_payload(ancestor_payload)
                        .await
                        .map_err(|e| eyre::eyre!("ancestor newPayload failed: {e}"))?;

                    match ancestor_status.status {
                        PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                            // Found a block the engine accepts, no need to go further back.
                            break;
                        }
                        PayloadStatusEnum::Invalid { .. } => {
                            warn!(
                                target: "consensus::debug-client",
                                %num,
                                ?ancestor_status,
                                "ancestor returned Invalid during backfill"
                            );
                            return Err(eyre::eyre!(
                                "ancestor block {num} returned Invalid during backfill"
                            ));
                        }
                        PayloadStatusEnum::Syncing => {
                            // Keep walking back.
                        }
                    }
                }

                // Submit ancestors oldest-to-newest (reverse of collection order).
                // Skip the last one since we already submitted it in the probe loop above.
                // Track hashes for ringbuffer continuity.
                let mut accepted_hashes = Vec::new();
                if ancestors.len() > 1 {
                    for ancestor in ancestors.into_iter().rev().skip(1) {
                        let ancestor_payload =
                            T::block_to_payload(SealedBlock::new_unhashed(ancestor));
                        let hash = ancestor_payload.block_hash();
                        let res = self
                            .engine_handle
                            .new_payload(ancestor_payload)
                            .await
                            .map_err(|e| eyre::eyre!("forward newPayload failed: {e}"))?;

                        if matches!(
                            res.status,
                            PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted
                        ) {
                            accepted_hashes.push(hash);
                        }
                    }
                }

                // Retry the original block.
                let retry_status = self
                    .engine_handle
                    .new_payload(payload)
                    .await
                    .map_err(|e| eyre::eyre!("retry newPayload failed: {e}"))?;

                if matches!(
                    retry_status.status,
                    PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted
                ) {
                    debug!(
                        target: "consensus::debug-client",
                        %block_number,
                        backfilled = accepted_hashes.len(),
                        "block accepted after backfill"
                    );

                    // Enqueue backfilled ancestor hashes to maintain ringbuffer continuity.
                    for hash in accepted_hashes {
                        previous_block_hashes.enqueue(hash);
                    }
                    previous_block_hashes.enqueue(block_hash);
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "block {block_number} still not accepted after backfill: {:?}",
                        retry_status.status
                    ))
                }
            }
            PayloadStatusEnum::Invalid { .. } => {
                warn!(
                    target: "consensus::debug-client",
                    %block_number,
                    %block_hash,
                    ?status,
                    "newPayload returned Invalid"
                );
                Err(eyre::eyre!("block {block_number} returned Invalid"))
            }
        }
    }
}

/// Looks up a block hash from the ring buffer at the given offset from the most recent entry.
///
/// Returns `None` if the buffer doesn't have enough entries to satisfy the offset.
fn get_hash_at_offset(buffer: &AllocRingBuffer<B256>, offset: usize) -> Option<B256> {
    buffer.len().checked_sub(offset + 1).and_then(|index| buffer.get(index).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_hash_at_offset() {
        let mut buffer: AllocRingBuffer<B256> = AllocRingBuffer::new(65);

        // Empty buffer returns None for any offset
        assert_eq!(get_hash_at_offset(&buffer, 0), None);
        assert_eq!(get_hash_at_offset(&buffer, 1), None);

        // Push hashes 0..65
        for i in 0..65u8 {
            buffer.enqueue(B256::with_last_byte(i));
        }

        // offset=0 should return the most recent (64)
        assert_eq!(get_hash_at_offset(&buffer, 0), Some(B256::with_last_byte(64)));

        // offset=32 (safe block) should return hash 32
        assert_eq!(get_hash_at_offset(&buffer, 32), Some(B256::with_last_byte(32)));

        // offset=64 (finalized block) should return hash 0 (the oldest)
        assert_eq!(get_hash_at_offset(&buffer, 64), Some(B256::with_last_byte(0)));

        // offset=65 exceeds buffer, should return None
        assert_eq!(get_hash_at_offset(&buffer, 65), None);
    }

    #[test]
    fn test_get_hash_at_offset_insufficient_entries() {
        let mut buffer: AllocRingBuffer<B256> = AllocRingBuffer::new(65);

        // With only 1 entry, only offset=0 works
        buffer.enqueue(B256::with_last_byte(1));
        assert_eq!(get_hash_at_offset(&buffer, 0), Some(B256::with_last_byte(1)));
        assert_eq!(get_hash_at_offset(&buffer, 1), None);
        assert_eq!(get_hash_at_offset(&buffer, 32), None);
        assert_eq!(get_hash_at_offset(&buffer, 64), None);

        // With 33 entries, offset=32 works but offset=64 doesn't
        for i in 2..=33u8 {
            buffer.enqueue(B256::with_last_byte(i));
        }
        assert_eq!(get_hash_at_offset(&buffer, 32), Some(B256::with_last_byte(1)));
        assert_eq!(get_hash_at_offset(&buffer, 64), None);
    }
}
