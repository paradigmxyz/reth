//! Sequence cache management for flashblocks.
//!
//! The `SequenceManager` maintains a ring buffer of recently completed flashblock sequences
//! and intelligently selects which sequence to build based on the local chain tip.

use crate::{
    sequence::{FlashBlockPendingSequence, SequenceExecutionOutcome},
    worker::BuildArgs,
    FlashBlock, FlashBlockCompleteSequence,
};
use alloy_eips::eip2718::WithEncoded;
use alloy_primitives::B256;
use reth_chain_state::ExecutedBlock;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives, Recovered, SignedTransaction};
use reth_revm::cached::CachedReads;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use tokio::sync::{broadcast, RwLock};
use tracing::*;

/// Maximum number of cached sequences in the ring buffer.
const CACHE_SIZE: usize = 3;
/// 200 ms flashblock time.
pub(crate) const FLASHBLOCK_BLOCK_TIME: u64 = 200;

/// Stores the current pending and completed flashblock sequences in a fixed-size ring buffer.
#[derive(Debug)]
pub(crate) struct SequenceCache<T: SignedTransaction> {
    /// Current pending sequence being built up from incoming flashblocks
    pending: FlashBlockPendingSequence,
    /// Cached recovered transactions for the pending sequence
    pending_transactions: Vec<WithEncoded<Recovered<T>>>,
    /// Ring buffer of recently completed sequences bundled with their decoded transactions (FIFO,
    /// size 3)
    completed_cache: AllocRingBuffer<(FlashBlockCompleteSequence, Vec<WithEncoded<Recovered<T>>>)>,
}

impl<T: SignedTransaction> SequenceCache<T> {
    /// Creates a new `SequenceCache`.
    fn new(cache_size: usize) -> Self {
        Self {
            pending: FlashBlockPendingSequence::new(),
            pending_transactions: Vec::new(),
            completed_cache: AllocRingBuffer::new(cache_size),
        }
    }
}

/// Manages flashblock sequences with caching support.
///
/// This struct handles:
/// - Tracking the current pending sequence
/// - Finding the best sequence to build based on local chain tip
/// - Broadcasting completed sequences to subscribers
#[derive(Debug)]
pub(crate) struct SequenceManager<P: PayloadTypes, T: SignedTransaction> {
    /// The sequence cache
    inner: RwLock<SequenceCache<T>>,
    /// Broadcast channel for completed sequences
    block_broadcaster: broadcast::Sender<FlashBlockCompleteSequence>,
    /// Handle to consensus engine
    engine_handle: ConsensusEngineHandle<P>,
}

impl<P: PayloadTypes, T: SignedTransaction, N: NodePrimitives> SequenceManager<P, T>
where
    P::BuiltPayload: BuiltPayload<Primitives = N>,
{
    /// Creates a new sequence manager.
    pub(crate) fn new(engine_handle: ConsensusEngineHandle<P>) -> Self {
        let (block_broadcaster, _) = broadcast::channel(128);
        Self {
            inner: RwLock::new(SequenceCache::new(CACHE_SIZE)),
            block_broadcaster,
            engine_handle,
        }
    }

    /// Returns the sender half of the flashblock sequence broadcast channel.
    pub(crate) const fn block_sequence_broadcaster(
        &self,
    ) -> &broadcast::Sender<FlashBlockCompleteSequence> {
        &self.block_broadcaster
    }

    /// Gets a subscriber to the flashblock sequences produced.
    pub(crate) fn subscribe_block_sequence(&self) -> crate::FlashBlockCompleteSequenceRx {
        self.block_broadcaster.subscribe()
    }

    /// Inserts a new flashblock into the pending sequence.
    ///
    /// When a flashblock with index 0 arrives (indicating a new block), the current
    /// pending sequence is finalized, cached, and broadcast immediately. If the sequence
    /// is later built on top of local tip, `on_build_complete()` will broadcast again
    /// with computed `state_root`.
    ///
    /// Transactions are recovered once and cached for reuse during block building.
    pub(crate) async fn insert_flashblock(&self, flashblock: FlashBlock) -> eyre::Result<()> {
        let recovered_txs: Vec<WithEncoded<Recovered<T>>> =
            flashblock.recover_transactions().collect::<Result<Vec<_>, _>>()?;

        if let Some(completed) = {
            let mut inner = self.inner.write().await;

            // If this starts a new block, finalize and cache the previous sequence BEFORE inserting
            let completed = if flashblock.index == 0 && inner.pending.count() > 0 {
                let completed = inner.pending.finalize()?;
                let block_number = completed.block_number();
                let parent_hash = completed.payload_base().parent_hash;

                trace!(
                    target: "flashblocks",
                    block_number,
                    %parent_hash,
                    cache_size = inner.completed_cache.len(),
                    "Caching completed flashblock sequence"
                );

                // Bundle completed sequence with its decoded transactions and push to cache
                // Ring buffer automatically evicts oldest entry when full
                let txs = std::mem::take(&mut inner.pending_transactions);
                inner.completed_cache.push((completed.clone(), txs));

                // ensure cache is wiped on new flashblock
                let _ = inner.pending.take_cached_reads();
                Some(completed)
            } else {
                None
            };

            inner.pending_transactions.extend(recovered_txs);
            inner.pending.insert(flashblock);
            completed
        } {
            // Broadcast immediately to consensus client (even without state_root)
            // This ensures sequences are forwarded during catch-up even if not buildable on
            // tip. ConsensusClient checks execution_outcome and skips
            // newPayload if state_root is zero.
            if self.block_broadcaster.receiver_count() > 0 {
                let _ = self.block_broadcaster.send(completed);
            }
        }

        Ok(())
    }

    /// Returns the current pending sequence for inspection.
    pub(crate) async fn pending_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner.pending.count()
    }

    /// Finds the next sequence to build and returns ready-to-use `BuildArgs`.
    ///
    /// Priority order:
    /// 1. Current pending sequence (if parent matches local tip)
    /// 2. Cached sequence with exact parent match
    ///
    /// Returns None if nothing is buildable right now.
    pub(crate) async fn next_buildable_args(
        &self,
        local_tip_hash: B256,
        local_tip_timestamp: u64,
    ) -> Option<BuildArgs<Vec<WithEncoded<Recovered<T>>>>> {
        // Try to find a buildable sequence
        let (
            base,
            last_flashblock_index,
            last_flashblock_hash,
            last_flashblock_state_root,
            transactions,
            cached_state,
            source_name,
        ) = {
            let mut inner = self.inner.write().await;

            // Priority 1: Try current pending sequence
            if let Some(base) =
                inner.pending.payload_base().filter(|b| b.parent_hash == local_tip_hash)
            {
                let cached_state = inner.pending.take_cached_reads().map(|r| (base.parent_hash, r));
                let last_fb = inner.pending.last_flashblock()?;
                let transactions = inner.pending_transactions.clone();
                // Extract Copy fields before dropping the guard
                (
                    base,
                    last_fb.index,
                    last_fb.diff.block_hash,
                    last_fb.diff.state_root,
                    transactions,
                    cached_state,
                    "pending",
                )
            }
            // Priority 2: Try cached sequence with exact parent match
            else if let Some((cached, txs)) = inner
                .completed_cache
                .iter()
                .find(|(c, _)| c.payload_base().parent_hash == local_tip_hash)
            {
                let base = cached.payload_base().clone();
                let last_fb = cached.last();
                let transactions = txs.clone();
                let cached_state = None;
                // Extract Copy fields before dropping the guard
                (
                    base,
                    last_fb.index,
                    last_fb.diff.block_hash,
                    last_fb.diff.state_root,
                    transactions,
                    cached_state,
                    "cached",
                )
            } else {
                return None;
            }
        };

        // Auto-detect when to compute state root: only if the builder didn't provide it (sent
        // B256::ZERO) and we're near the expected final flashblock index.
        //
        // Background: Each block period receives multiple flashblocks at regular intervals.
        // The sequencer sends an initial "base" flashblock at index 0 when a new block starts,
        // then subsequent flashblocks are produced every FLASHBLOCK_BLOCK_TIME intervals (200ms).
        //
        // Examples with different block times:
        // - Base (2s blocks):    expect 2000ms / 200ms = 10 intervals → Flashblocks: index 0 (base)
        //   + indices 1-10 = potentially 11 total
        //
        // - Unichain (1s blocks): expect 1000ms / 200ms = 5 intervals → Flashblocks: index 0 (base)
        //   + indices 1-5 = potentially 6 total
        //
        // Why compute at N-1 instead of N:
        // 1. Timing variance in flashblock producing time may mean only N flashblocks were produced
        //    instead of N+1 (missing the final one). Computing at N-1 ensures we get the state root
        //    for most common cases.
        //
        // 2. The +1 case (index 0 base + N intervals): If all N+1 flashblocks do arrive, we'll
        //    still calculate state root for flashblock N, which sacrifices a little performance but
        //    still ensures correctness for common cases.
        //
        // Note: Pathological cases may result in fewer flashblocks than expected (e.g., builder
        // downtime, flashblock execution exceeding timing budget). When this occurs, we won't
        // compute the state root, causing FlashblockConsensusClient to lack precomputed state for
        // engine_newPayload. This is safe: we still have op-node as backstop to maintain
        // chain progression.
        let block_time_ms = (base.timestamp - local_tip_timestamp) * 1000;
        let expected_final_flashblock = block_time_ms / FLASHBLOCK_BLOCK_TIME;
        let compute_state_root = last_flashblock_state_root.is_zero() &&
            last_flashblock_index >= expected_final_flashblock.saturating_sub(1);

        trace!(
            target: "flashblocks",
            block_number = base.block_number,
            source = source_name,
            flashblock_index = last_flashblock_index,
            expected_final_flashblock,
            state_root_is_zero = last_flashblock_state_root.is_zero(),
            will_compute_state_root = compute_state_root,
            "Building from flashblock sequence"
        );

        Some(BuildArgs {
            base,
            transactions,
            cached_state,
            last_flashblock_index,
            last_flashblock_hash,
            compute_state_root,
        })
    }

    /// Records the cache reads of building the pending sequence.
    ///
    /// Updates execution outcome and cached reads.
    pub(crate) async fn on_build_complete(&self, parent_hash: B256, cached_reads: CachedReads) {
        let mut inner = self.inner.write().await;

        // Update pending sequence with cache reads. Skip if sequence is already completed
        if inner.pending.payload_base().is_some_and(|base| base.parent_hash == parent_hash) {
            inner.pending.set_cached_reads(cached_reads);
            trace!(
                target: "flashblocks",
                block_number = inner.pending.block_number(),
                "Updated pending sequence with build results"
            );
        }
    }

    /// Updates the sequence manager after state root computation.
    ///
    /// 1. Commits the flashblocks sequence to the engine state tree for pre-warming.
    /// 2. Re-broadcasts flashblocks sequence with execution outcome for flashblocks consensus with
    ///    the computed `state_root`, allowing the consensus client to submit via
    ///    `engine_newPayload`.
    pub(crate) async fn on_state_root_complete(
        &self,
        parent_hash: B256,
        computed_block: ExecutedBlock<N>,
    ) {
        // Extract execution outcome
        let execution_outcome = Some(SequenceExecutionOutcome {
            block_hash: computed_block.recovered_block().hash(),
            state_root: computed_block.recovered_block().state_root(),
        });

        // Submit executed block with trie updates to engine state tree
        self.submit_executed_block(computed_block);

        let to_broadcast = {
            // Update pending sequence with execution outcome
            let mut inner = self.inner.write().await;
            if inner.pending.payload_base().is_some_and(|base| base.parent_hash == parent_hash) {
                inner.pending.set_execution_outcome(execution_outcome);
                None
            }
            // Check if this completed sequence in cache and re-broadcast with execution outcome
            else if let Some((cached, _)) = inner
                .completed_cache
                .iter_mut()
                .find(|(c, _)| c.payload_base().parent_hash == parent_hash)
            {
                // Only re-broadcast if we computed new information (state_root was missing).
                // If sequencer already provided state_root, we already broadcast in
                // insert_flashblock, so skip re-broadcast to avoid duplicate FCU
                // calls.
                let needs_rebroadcast = cached.execution_outcome().is_none();
                cached.set_execution_outcome(execution_outcome);
                (needs_rebroadcast && self.block_broadcaster.receiver_count() > 0)
                    .then(|| cached.clone())
            } else {
                None
            }
        };

        if let Some(cached) = to_broadcast {
            trace!(
                target: "flashblocks",
                block_number = cached.block_number(),
                "Re-broadcasting sequence with computed state_root"
            );
            let _ = self.block_broadcaster.send(cached);
        };
    }

    /// Submit the `ExecutedBlock` to pre-warm the engine state tree. Note that executed blocks
    /// require proper trie updates to avoid corrupting the engine's trie input computation.
    pub(crate) fn submit_executed_block(&self, executed_block: ExecutedBlock<N>) {
        let num_hash = executed_block.recovered_block().num_hash();
        self.engine_handle.send_executed_flashblocks_sequence(executed_block);
        debug!(
            target: "flashblocks",
            block_number = num_hash.number,
            block_hash = %num_hash.hash,
            "Submitted executed flashblocks sequence to engine"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestFlashBlockFactory;
    use alloy_primitives::B256;
    use op_alloy_consensus::OpTxEnvelope;
    use reth_optimism_payload_builder::OpPayloadTypes;

    #[tokio::test]
    async fn test_sequence_manager_new() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let inner = manager.inner.read().await;
        assert_eq!(inner.pending.count(), 0);
    }

    #[tokio::test]
    async fn test_insert_flashblock_creates_pending_sequence() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        let fb0 = factory.flashblock_at(0).build();
        manager.insert_flashblock(fb0).await.unwrap();

        let inner = manager.inner.read().await;
        assert_eq!(inner.pending.count(), 1);
        assert_eq!(inner.pending.block_number(), Some(100));
    }

    #[tokio::test]
    async fn test_insert_flashblock_caches_completed_sequence() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        // Build first sequence
        let fb0 = factory.flashblock_at(0).build();
        manager.insert_flashblock(fb0.clone()).await.unwrap();

        let fb1 = factory.flashblock_after(&fb0).build();
        manager.insert_flashblock(fb1).await.unwrap();

        // Insert new base (index 0) which should finalize and cache previous sequence
        let fb2 = factory.flashblock_for_next_block(&fb0).build();
        manager.insert_flashblock(fb2).await.unwrap();

        // New sequence should be pending
        let inner = manager.inner.read().await;
        assert_eq!(inner.pending.count(), 1);
        assert_eq!(inner.pending.block_number(), Some(101));
        assert_eq!(inner.completed_cache.len(), 1);
        let (cached_sequence, _txs) = inner.completed_cache.get(0).unwrap();
        assert_eq!(cached_sequence.block_number(), 100);
    }

    #[tokio::test]
    async fn test_next_buildable_args_returns_none_when_empty() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let local_tip_hash = B256::random();
        let local_tip_timestamp = 1000;

        let args = manager.next_buildable_args(local_tip_hash, local_tip_timestamp).await;
        assert!(args.is_none());
    }

    #[tokio::test]
    async fn test_next_buildable_args_matches_pending_parent() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        let fb0 = factory.flashblock_at(0).build();
        let parent_hash = fb0.base.as_ref().unwrap().parent_hash;
        manager.insert_flashblock(fb0).await.unwrap();

        let args = manager.next_buildable_args(parent_hash, 1000000).await;
        assert!(args.is_some());

        let build_args = args.unwrap();
        assert_eq!(build_args.last_flashblock_index, 0);
    }

    #[tokio::test]
    async fn test_next_buildable_args_returns_none_when_parent_mismatch() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        let fb0 = factory.flashblock_at(0).build();
        manager.insert_flashblock(fb0).await.unwrap();

        // Use different parent hash
        let wrong_parent = B256::random();
        let args = manager.next_buildable_args(wrong_parent, 1000000).await;
        assert!(args.is_none());
    }

    #[tokio::test]
    async fn test_next_buildable_args_prefers_pending_over_cached() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        // Create and finalize first sequence
        let fb0 = factory.flashblock_at(0).build();
        manager.insert_flashblock(fb0.clone()).await.unwrap();

        // Create new sequence (finalizes previous)
        let fb1 = factory.flashblock_for_next_block(&fb0).build();
        let parent_hash = fb1.base.as_ref().unwrap().parent_hash;
        manager.insert_flashblock(fb1).await.unwrap();

        // Request with first sequence's parent (should find cached)
        let args = manager.next_buildable_args(parent_hash, 1000000).await;
        assert!(args.is_some());
    }

    #[tokio::test]
    async fn test_next_buildable_args_finds_cached_sequence() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        // Build and cache first sequence
        let fb0 = factory.flashblock_at(0).build();
        let parent_hash = fb0.base.as_ref().unwrap().parent_hash;
        manager.insert_flashblock(fb0.clone()).await.unwrap();

        // Start new sequence to finalize first
        let fb1 = factory.flashblock_for_next_block(&fb0).build();
        manager.insert_flashblock(fb1.clone()).await.unwrap();

        // Clear pending by starting another sequence
        let fb2 = factory.flashblock_for_next_block(&fb1).build();
        manager.insert_flashblock(fb2).await.unwrap();

        // Request first sequence's parent - should find in cache
        let args = manager.next_buildable_args(parent_hash, 1000000).await;
        assert!(args.is_some());
    }

    #[tokio::test]
    async fn test_compute_state_root_logic_near_expected_final() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let block_time = 2u64;
        let factory = TestFlashBlockFactory::new().with_block_time(block_time);

        // Create sequence with zero state root (needs computation)
        let fb0 = factory.flashblock_at(0).state_root(B256::ZERO).build();
        let parent_hash = fb0.base.as_ref().unwrap().parent_hash;
        let base_timestamp = fb0.base.as_ref().unwrap().timestamp;
        manager.insert_flashblock(fb0.clone()).await.unwrap();

        // Add flashblocks up to expected final index (2000ms / 200ms = 10)
        for i in 1..=9 {
            let fb = factory.flashblock_after(&fb0).index(i).state_root(B256::ZERO).build();
            manager.insert_flashblock(fb).await.unwrap();
        }

        // Request with proper timing - should compute state root for index 9
        let args = manager.next_buildable_args(parent_hash, base_timestamp - block_time).await;
        assert!(args.is_some());
        assert!(args.unwrap().compute_state_root);
    }

    #[tokio::test]
    async fn test_no_compute_state_root_when_provided_by_sequencer() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let block_time = 2u64;
        let factory = TestFlashBlockFactory::new().with_block_time(block_time);

        // Create sequence with non-zero state root (provided by sequencer)
        let fb0 = factory.flashblock_at(0).state_root(B256::random()).build();
        let parent_hash = fb0.base.as_ref().unwrap().parent_hash;
        let base_timestamp = fb0.base.as_ref().unwrap().timestamp;
        manager.insert_flashblock(fb0).await.unwrap();

        let args = manager.next_buildable_args(parent_hash, base_timestamp - block_time).await;
        assert!(args.is_some());
        assert!(!args.unwrap().compute_state_root);
    }

    #[tokio::test]
    async fn test_cache_ring_buffer_evicts_oldest() {
        let (engine_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let engine_handle = ConsensusEngineHandle::<OpPayloadTypes>::new(engine_tx);
        let manager: SequenceManager<OpPayloadTypes, OpTxEnvelope> =
            SequenceManager::new(engine_handle);
        let factory = TestFlashBlockFactory::new();

        // Fill cache with 4 sequences (cache size is 3, so oldest should be evicted)
        let mut last_fb = factory.flashblock_at(0).build();
        manager.insert_flashblock(last_fb.clone()).await.unwrap();

        for _ in 0..3 {
            last_fb = factory.flashblock_for_next_block(&last_fb).build();
            manager.insert_flashblock(last_fb.clone()).await.unwrap();
        }

        // The first sequence should have been evicted, so we can't build it
        let first_parent = factory.flashblock_at(0).build().base.unwrap().parent_hash;
        let args = manager.next_buildable_args(first_parent, 1000000).await;
        // Should not find it (evicted from ring buffer)
        assert!(args.is_none());
    }
}
