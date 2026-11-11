use super::{
    BlockState, ExecutedBlock, InMemoryCanonicalBlocksRemoval, InMemoryState, InMemoryStateWriter,
    NewCanonicalChain,
};
use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications,
    ChainInfoHeaders, ChainInfoTracker,
};
use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, TxHash, B256};
use reth_chainspec::ChainInfo;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::RecoveredBlockAndExecutionOutput;
use reth_primitives_traits::{
    BlockBody as _, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::{broadcast, watch};

/// Size of the broadcast channel used to notify canonical state events.
const CANON_STATE_NOTIFICATION_CHANNEL_SIZE: usize = 256;

/// Read handle to a canonical chain tracking: the chain info, canonical state notifications, and
/// the canonical view of an [`InMemoryState`].
///
/// Every block lookup on this type only returns canonical blocks. Lookups across all tracked
/// blocks are on [`Self::in_memory_state`]. The one exception is the pending block, which is not
/// canonical, see [`Self::pending_state`].
///
/// Clones read the same tracking. The chain info and the notifications belong to the value
/// [`Self::new`] created and to its clones, and only change through the
/// [`CanonicalInMemoryStateWriter`] of that value. The blocks belong to the [`InMemoryState`]:
/// every handle of it reads them, and they change through its [`InMemoryStateWriter`], see
/// [`Self::new`].
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState<N: NodePrimitives = EthPrimitives> {
    inner: Arc<CanonicalInMemoryStateInner<N>>,
}

impl<N: NodePrimitives> CanonicalInMemoryState<N> {
    /// Creates the canonical tracking for `in_memory_state`, starting from `chain_info_headers`.
    ///
    /// The blocks are shared with every other handle of `in_memory_state`, canonical trackings
    /// created by other calls for the same in-memory state included. The chain info and the
    /// notifications are this value's own: they stay at `chain_info_headers` until the
    /// [`CanonicalInMemoryStateWriter`] created for this value changes them. Only one tracking of
    /// an in-memory state can have a writer, so the chain info of every other one never changes
    /// while its block lookups follow the shared blocks.
    pub fn new(
        chain_info_headers: ChainInfoHeaders<N::BlockHeader>,
        in_memory_state: InMemoryState<N>,
    ) -> Self {
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);
        Self {
            inner: Arc::new(CanonicalInMemoryStateInner {
                chain_info_tracker: ChainInfoTracker::new(chain_info_headers),
                in_memory_state,
                canon_state_notification_sender,
            }),
        }
    }

    /// Returns the hash of the canonical in-memory block with the given number.
    pub fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.inner.in_memory_state.read(|blocks| blocks.canonical().hash_by_number(number))
    }

    /// Returns the header of the canonical in-memory block with the given hash.
    pub fn header_by_hash(&self, hash: B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.state_by_hash(hash)
            .map(|state| state.block_ref().recovered_block().clone_sealed_header())
    }

    /// Returns the state of the canonical in-memory block with the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.read(|blocks| blocks.canonical().get(&hash).cloned())
    }

    /// Returns the state of the canonical in-memory block with the given number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.read(|blocks| blocks.canonical().by_number(number).cloned())
    }

    /// Returns the state of the highest canonical in-memory block.
    pub fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.read(|blocks| blocks.canonical().head().cloned())
    }

    /// Returns the state of the pending block: the block executed last on top of the current
    /// canonical head, if there is one.
    ///
    /// The pending block is tracked but not canonical.
    pub fn pending_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.read(|blocks| blocks.pending_state().cloned())
    }

    /// Returns the `BlockNumHash` of the pending block, see [`Self::pending_state`].
    pub fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        self.pending_state()
            .map(|state| BlockNumHash { number: state.number(), hash: state.hash() })
    }

    /// Returns the current `ChainInfo`.
    pub fn chain_info(&self) -> ChainInfo {
        self.inner.chain_info_tracker.chain_info()
    }

    /// Returns the latest canonical block number.
    pub fn get_canonical_block_number(&self) -> u64 {
        self.inner.chain_info_tracker.get_canonical_block_number()
    }

    /// Returns the `BlockNumHash` of the safe head.
    pub fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_safe_num_hash()
    }

    /// Returns the `BlockNumHash` of the finalized head.
    pub fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_finalized_num_hash()
    }

    /// Returns the timestamp of the last received update.
    pub fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_forkchoice_update_received_at()
    }

    /// Canonical head getter.
    pub fn get_canonical_head(&self) -> SealedHeader<N::BlockHeader> {
        self.inner.chain_info_tracker.get_canonical_head()
    }

    /// Finalized header getter.
    pub fn get_finalized_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_finalized_header()
    }

    /// Safe header getter.
    pub fn get_safe_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_safe_header()
    }

    /// Persisted block `BlockNumHash` getter.
    pub fn get_persisted_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_persisted_num_hash()
    }

    /// Returns the `SealedHeader` corresponding to the pending state.
    pub fn pending_sealed_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.pending_state().map(|h| h.block_ref().recovered_block().clone_sealed_header())
    }

    /// Returns the `Header` corresponding to the pending state.
    pub fn pending_header(&self) -> Option<N::BlockHeader> {
        self.pending_sealed_header().map(|sealed_header| sealed_header.unseal())
    }

    /// Returns the `SealedBlock` corresponding to the pending state.
    pub fn pending_block(&self) -> Option<SealedBlock<N::Block>> {
        self.pending_state()
            .map(|block_state| block_state.block_ref().recovered_block().sealed_block().clone())
    }

    /// Returns the `RecoveredBlock` corresponding to the pending state.
    pub fn pending_recovered_block(&self) -> Option<Arc<RecoveredBlock<N::Block>>> {
        self.pending_state().map(|block_state| Arc::clone(&block_state.block_ref().recovered_block))
    }

    /// Returns the pending recovered block and its execution output, which contains the receipts.
    pub fn pending_block_and_receipts(
        &self,
    ) -> Option<RecoveredBlockAndExecutionOutput<N::Block, N::Receipt>> {
        self.pending_state().map(|block_state| {
            RecoveredBlockAndExecutionOutput::new(
                Arc::clone(&block_state.block_ref().recovered_block),
                Arc::clone(&block_state.block_ref().execution_output),
            )
        })
    }

    /// Subscribe to new blocks events.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications<N> {
        self.inner.canon_state_notification_sender.subscribe()
    }

    /// Subscribe to new safe block events.
    pub fn subscribe_safe_block(&self) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_safe_block()
    }

    /// Subscribe to new finalized block events.
    pub fn subscribe_finalized_block(
        &self,
    ) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_finalized_block()
    }

    /// Subscribe to new persisted block events.
    pub fn subscribe_persisted_block(&self) -> watch::Receiver<Option<BlockNumHash>> {
        self.inner.chain_info_tracker.subscribe_persisted_block()
    }

    /// Returns an iterator over all __canonical blocks__ in the in-memory state, from newest to
    /// oldest (highest to lowest).
    ///
    /// This iterator contains a snapshot of the in-memory state at the time of the call.
    pub fn canonical_chain(&self) -> impl Iterator<Item = Arc<BlockState<N>>> {
        self.head_state().into_iter().flat_map(|head| head.iter())
    }

    /// Returns the transaction with the given hash if it is part of a canonical in-memory block.
    pub fn transaction_by_hash(&self, hash: TxHash) -> Option<N::SignedTx> {
        for block_state in self.canonical_chain() {
            if let Some(tx) =
                block_state.block_ref().recovered_block().body().transaction_by_hash(&hash)
            {
                return Some(tx.clone())
            }
        }
        None
    }

    /// Returns the transaction with the given hash and its [`TransactionMeta`] if it is part of a
    /// canonical in-memory block.
    pub fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        for block_state in self.canonical_chain() {
            if let Some(indexed) = block_state.find_indexed(tx_hash) {
                return Some((indexed.tx().clone(), indexed.meta()));
            }
        }
        None
    }

    /// Returns the `BlockNumHash` of the canonical head.
    pub fn get_canonical_num_hash(&self) -> BlockNumHash {
        self.inner.chain_info_tracker.get_canonical_num_hash()
    }

    /// Returns the in-memory state this is the canonical view of.
    pub fn in_memory_state(&self) -> &InMemoryState<N> {
        &self.inner.in_memory_state
    }
}

/// Write access to a [`CanonicalInMemoryState`] and to the blocks of its [`InMemoryState`].
///
/// Holding it is the only way to change the chain info of its [`CanonicalInMemoryState`] and to
/// send its canonical state notifications. It holds the only [`InMemoryStateWriter`] of the
/// in-memory state, so the blocks only change through it as well. It cannot be cloned.
///
/// The methods that change the canonical head change the canonical blocks and the canonical head
/// of the chain info while they hold the write lock of the in-memory state: whoever reads the
/// changed blocks and the canonical head afterwards, in this order, reads the head that belongs to
/// those blocks.
#[derive(Debug)]
pub struct CanonicalInMemoryStateWriter<N: NodePrimitives = EthPrimitives> {
    canonical_in_memory_state: CanonicalInMemoryState<N>,
    in_memory_state_writer: InMemoryStateWriter<N>,
}

impl<N: NodePrimitives> CanonicalInMemoryStateWriter<N> {
    /// Creates the writer for `canonical_in_memory_state`.
    ///
    /// # Panics
    ///
    /// - If `in_memory_state_writer` does not write to the in-memory state that
    ///   `canonical_in_memory_state` reads.
    /// - If the in-memory state has canonical blocks and the highest of them is not the canonical
    ///   head of `canonical_in_memory_state`.
    pub fn new(
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        in_memory_state_writer: InMemoryStateWriter<N>,
    ) -> Self {
        assert!(
            in_memory_state_writer.writes_to(canonical_in_memory_state.in_memory_state()),
            "the writer does not write to the in-memory state of the canonical in-memory state"
        );
        let canonical_head = canonical_in_memory_state.get_canonical_num_hash();
        assert!(
            canonical_in_memory_state
                .head_state()
                .is_none_or(|head_state| head_state.hash() == canonical_head.hash),
            "the highest canonical in-memory block is not the canonical head of the chain info"
        );
        Self { canonical_in_memory_state, in_memory_state_writer }
    }

    /// Returns the read handle this writes to.
    pub const fn canonical_in_memory_state(&self) -> &CanonicalInMemoryState<N> {
        &self.canonical_in_memory_state
    }

    /// Tracks a newly executed block.
    ///
    /// The block is inserted into the non-canonical section unless it is already tracked. It
    /// becomes the pending block if it is built on the canonical head, otherwise the pending block
    /// is unchanged. Returns `true` if the block was inserted, `false` if it was already tracked.
    /// Nothing else has to be updated afterwards.
    pub fn insert_executed(&mut self, block: ExecutedBlock<N>) -> bool {
        // The canonical head only changes through this writer, so it cannot change before the
        // block is inserted.
        let canonical_head = self.canonical_in_memory_state.get_canonical_num_hash();
        if block.recovered_block().parent_hash() == canonical_head.hash {
            self.in_memory_state_writer.update(|blocks| blocks.insert_pending(block))
        } else {
            self.in_memory_state_writer.insert_executed(block)
        }
    }

    /// Applies `new_chain` like [`InMemoryStateWriter::update_chain`] and makes its tip the
    /// canonical head of the chain info.
    ///
    /// Clears the pending block. Does not send a notification. Nothing else has to be updated
    /// afterwards.
    pub fn update_chain(&mut self, new_chain: NewCanonicalChain<N>) {
        let canonical_head = new_chain.tip().clone_sealed_header();
        let chain_info_tracker = &self.canonical_in_memory_state.inner.chain_info_tracker;
        self.in_memory_state_writer.update(|blocks| {
            blocks.update_chain(new_chain);
            chain_info_tracker.set_canonical_head(canonical_head);
        })
    }

    /// Makes `new_head`, a canonical ancestor of the canonical head, the canonical head.
    ///
    /// The canonical blocks above it are moved to the non-canonical section. If `new_head` is not
    /// canonical in memory because it is persisted, it becomes the only canonical in-memory
    /// block. Clears the pending block. Nothing else has to be updated afterwards.
    pub fn unwind_canonical_head(&mut self, new_head: ExecutedBlock<N>) {
        let canonical_head = new_head.recovered_block().clone_sealed_header();
        let chain_info_tracker = &self.canonical_in_memory_state.inner.chain_info_tracker;
        self.in_memory_state_writer.update(|blocks| {
            blocks.set_canonical_chain(vec![new_head]);
            chain_info_tracker.set_canonical_head(canonical_head);
        })
    }

    /// Makes the persisted block `persisted_head` the canonical head and moves every canonical
    /// in-memory block to the non-canonical section.
    ///
    /// Clears the pending block. Nothing else has to be updated afterwards.
    pub fn demote_canonical_chain(&mut self, persisted_head: SealedHeader<N::BlockHeader>) {
        let chain_info_tracker = &self.canonical_in_memory_state.inner.chain_info_tracker;
        self.in_memory_state_writer.update(|blocks| {
            blocks.demote_canonical_chain();
            chain_info_tracker.set_canonical_head(persisted_head);
        })
    }

    /// Removes all blocks of both sections and makes the persisted block `persisted_head` the
    /// canonical head.
    ///
    /// Clears the pending block. Nothing else has to be updated afterwards.
    pub fn clear(&mut self, persisted_head: SealedHeader<N::BlockHeader>) {
        let chain_info_tracker = &self.canonical_in_memory_state.inner.chain_info_tracker;
        let removed = self.in_memory_state_writer.update(|blocks| {
            let removed = blocks.clear();
            chain_info_tracker.set_canonical_head(persisted_head);
            removed
        });
        // The removed blocks are dropped here, after the lock is released.
        drop(removed);
    }

    /// See [`InMemoryStateWriter::remove_canonical_until`]. The chain info is unchanged.
    pub fn remove_canonical_until(
        &mut self,
        persisted_hash: B256,
        remove_until: BlockNumber,
    ) -> InMemoryCanonicalBlocksRemoval {
        self.in_memory_state_writer.remove_canonical_until(persisted_hash, remove_until)
    }

    /// See [`InMemoryStateWriter::prune_conflicting_with_finalized`]. The chain info is unchanged.
    pub fn prune_conflicting_with_finalized(&mut self, finalized: BlockNumHash) -> usize {
        self.in_memory_state_writer.prune_conflicting_with_finalized(finalized)
    }

    /// Sets the safe block.
    pub fn set_safe(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state.inner.chain_info_tracker.set_safe(header);
    }

    /// Sets the finalized block.
    pub fn set_finalized(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state.inner.chain_info_tracker.set_finalized(header);
    }

    /// Sets the last persisted block.
    pub fn set_persisted(&mut self, num_hash: BlockNumHash) {
        self.canonical_in_memory_state.inner.chain_info_tracker.set_persisted(num_hash);
    }

    /// Records that a forkchoice update was received now.
    pub fn on_forkchoice_update_received(&mut self) {
        self.canonical_in_memory_state.inner.chain_info_tracker.on_forkchoice_update_received();
    }

    /// Sends `event` to all subscribers of canonical state notifications.
    pub fn notify_canon_state(&mut self, event: CanonStateNotification<N>) {
        // Sending only fails if nobody is subscribed.
        self.canonical_in_memory_state.inner.canon_state_notification_sender.send(event).ok();
    }
}

/// The canonical chain tracking shared by the clones of a [`CanonicalInMemoryState`].
#[derive(Debug)]
struct CanonicalInMemoryStateInner<N: NodePrimitives> {
    /// Tracks the canonical head, the safe block, the finalized block and the persisted block.
    chain_info_tracker: ChainInfoTracker<N>,
    /// The executed blocks that are not on disk yet.
    in_memory_state: InMemoryState<N>,
    /// A broadcast stream that emits events when the canonical chain is updated.
    canon_state_notification_sender: CanonStateNotificationSender<N>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, InMemoryStateBlockCounts};
    use rand::Rng;

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Creates the canonical tracking of an empty in-memory state, starting from
    /// `canonical_head`, and its writer.
    fn canonical_in_memory_state_with_head(
        canonical_head: SealedHeader,
    ) -> (CanonicalInMemoryState, CanonicalInMemoryStateWriter) {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders { canonical_head, safe: None, finalized: None },
            in_memory_state,
        );
        let canonical_in_memory_state_writer = CanonicalInMemoryStateWriter::new(
            canonical_in_memory_state.clone(),
            in_memory_state_writer,
        );
        (canonical_in_memory_state, canonical_in_memory_state_writer)
    }

    fn canonical_in_memory_state() -> (CanonicalInMemoryState, CanonicalInMemoryStateWriter) {
        canonical_in_memory_state_with_head(SealedHeader::default())
    }

    fn assert_invariants(canonical_in_memory_state: &CanonicalInMemoryState) {
        canonical_in_memory_state.in_memory_state().read(|blocks| blocks.assert_invariants());
    }

    fn block_counts(canonical_in_memory_state: &CanonicalInMemoryState) -> (usize, usize) {
        let InMemoryStateBlockCounts { canonical, non_canonical } =
            canonical_in_memory_state.in_memory_state().block_counts();
        (canonical, non_canonical)
    }

    #[test]
    fn every_operation_keeps_sections_and_indexes_consistent() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks[1..] {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        assert_invariants(&canonical_in_memory_state);

        // Block 1 is tracked last and has to be linked to the blocks built on it, which are
        // tracked already.
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[..3].to_vec() });
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(
            canonical_in_memory_state
                .in_memory_state()
                .state_by_hash(hash(&blocks[4]))
                .unwrap()
                .chain()
                .count(),
            5
        );

        let fork = builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let fork_child = builder.get_executed_block_with_number(4, hash(&fork));
        canonical_in_memory_state_writer.insert_executed(fork.clone());
        canonical_in_memory_state_writer.insert_executed(fork_child.clone());
        assert_invariants(&canonical_in_memory_state);
        assert!(
            canonical_in_memory_state.pending_state().is_none(),
            "the fork is not built on the canonical head"
        );

        canonical_in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork.clone(), fork_child.clone()],
            old: vec![blocks[2].clone()],
        });
        assert_invariants(&canonical_in_memory_state);
        canonical_in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: blocks[2..].to_vec(),
            old: vec![fork, fork_child],
        });
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(block_counts(&canonical_in_memory_state), (5, 2));

        // A tracked block that is not built on the canonical head does not become pending.
        assert!(
            !canonical_in_memory_state_writer.insert_executed(blocks[0].clone()),
            "already tracked"
        );
        assert!(canonical_in_memory_state.pending_state().is_none());
        let persisted = blocks[1].recovered_block().num_hash();
        assert_eq!(
            canonical_in_memory_state_writer
                .remove_canonical_until(persisted.hash, persisted.number),
            InMemoryCanonicalBlocksRemoval::Removed(2)
        );
        assert_invariants(&canonical_in_memory_state);
        assert!(canonical_in_memory_state_writer.insert_executed(blocks[0].clone()));
        assert!(
            canonical_in_memory_state.pending_state().is_none(),
            "a removed block does not become pending"
        );
        assert_invariants(&canonical_in_memory_state);

        assert_eq!(
            canonical_in_memory_state_writer
                .prune_conflicting_with_finalized(blocks[2].recovered_block().num_hash()),
            3
        );
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(block_counts(&canonical_in_memory_state), (3, 0));

        canonical_in_memory_state_writer
            .insert_executed(builder.get_executed_block_with_number(6, hash(&blocks[4])));
        assert!(canonical_in_memory_state.pending_state().is_some());
        canonical_in_memory_state_writer
            .demote_canonical_chain(blocks[1].recovered_block().clone_sealed_header());
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(block_counts(&canonical_in_memory_state), (0, 4));
        assert!(canonical_in_memory_state.pending_state().is_none());

        canonical_in_memory_state_writer.clear(blocks[1].recovered_block().clone_sealed_header());
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(block_counts(&canonical_in_memory_state), (0, 0));
    }

    #[test]
    fn reinserting_disk_blocks_does_not_extend_the_canonical_chain() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        // Blocks 1 to 3 are persisted and not tracked, 4 and 5 are the in-memory canonical chain.
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[3..].to_vec() });

        // Blocks 2 and 3 are tracked again, one by one, while readers keep seeing the in-memory
        // canonical chain unchanged.
        for block in &blocks[1..3] {
            canonical_in_memory_state_writer.insert_executed(block.clone());
            assert_invariants(&canonical_in_memory_state);
            assert_eq!(canonical_in_memory_state.head_state().unwrap().chain().count(), 2);
            assert_eq!(
                canonical_in_memory_state.canonical_chain().count(),
                block_counts(&canonical_in_memory_state).0
            );
        }

        // A reorg to a fork off block 1 replaces blocks 2 to 5.
        let mut parent = hash(&blocks[0]);
        let fork = (2..=6)
            .map(|number| {
                let block = builder.get_executed_block_with_number(number, parent);
                parent = hash(&block);
                block
            })
            .collect::<Vec<_>>();
        for block in &fork {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Reorg { new: fork, old: blocks[1..].to_vec() });
        assert_invariants(&canonical_in_memory_state);
        assert_eq!(block_counts(&canonical_in_memory_state).0, 5);
        assert_eq!(canonical_in_memory_state.head_state().unwrap().chain().count(), 5);

        // The replaced blocks stay tracked, linked to each other down to block 2.
        let old_tip =
            canonical_in_memory_state.in_memory_state().state_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(old_tip.chain().count(), 4);
        assert_eq!(old_tip.anchor(), blocks[0].recovered_block().num_hash());
    }

    #[test]
    fn test_in_memory_state_impl_state_by_hash() {
        let number = rand::rng().random::<u64>();
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![block.clone()] });

        assert_eq!(
            canonical_in_memory_state.state_by_hash(hash(&block)),
            Some(Arc::new(BlockState::new(block)))
        );
        assert_eq!(canonical_in_memory_state.state_by_hash(B256::random()), None);
    }

    #[test]
    fn test_in_memory_state_impl_state_by_number() {
        let number = rand::rng().random_range(0..u64::MAX);
        let mut test_block_builder = TestBlockBuilder::eth();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![block.clone()] });

        assert_eq!(canonical_in_memory_state.hash_by_number(number), Some(hash(&block)));
        assert_eq!(
            canonical_in_memory_state.state_by_number(number),
            Some(Arc::new(BlockState::new(block)))
        );
        assert_eq!(canonical_in_memory_state.state_by_number(number + 1), None);
    }

    #[test]
    fn test_in_memory_state_impl_head_state() {
        let mut test_block_builder = TestBlockBuilder::eth();
        let block1 = test_block_builder.get_executed_block_with_number(1, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(2, hash(&block1));
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![block1, block2.clone()] });

        let head_state = canonical_in_memory_state.head_state().unwrap();

        assert_eq!(head_state.hash(), hash(&block2));
        assert_eq!(head_state.number(), 2);
    }

    #[test]
    fn test_in_memory_state_impl_pending_state() {
        let pending_number = rand::rng().random::<u64>();
        let mut test_block_builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let pending = test_block_builder.get_executed_block_with_number(
            pending_number,
            canonical_in_memory_state.get_canonical_head().hash(),
        );
        canonical_in_memory_state_writer.insert_executed(pending.clone());

        let result = canonical_in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.hash(), hash(&pending));
        assert_eq!(actual_pending_state.block_ref().recovered_block().number, pending_number);
    }

    #[test]
    fn test_in_memory_state_impl_no_pending_state() {
        let (canonical_in_memory_state, _canonical_in_memory_state_writer) =
            canonical_in_memory_state();

        assert_eq!(canonical_in_memory_state.pending_state(), None);
    }

    #[test]
    fn test_in_memory_state_chain_update() {
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let mut test_block_builder = TestBlockBuilder::eth();
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone()] };
        canonical_in_memory_state_writer.update_chain(chain);
        assert_eq!(
            canonical_in_memory_state.head_state().unwrap().hash(),
            block1.recovered_block().hash()
        );
        assert_eq!(
            canonical_in_memory_state.state_by_number(0).unwrap().hash(),
            block1.recovered_block().hash()
        );

        let chain =
            NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1.clone()] };
        canonical_in_memory_state_writer.update_chain(chain);
        assert_eq!(
            canonical_in_memory_state.head_state().unwrap().hash(),
            block2.recovered_block().hash()
        );
        assert_eq!(
            canonical_in_memory_state.state_by_number(0).unwrap().hash(),
            block2.recovered_block().hash()
        );

        // The reorged block is no longer canonical but stays tracked.
        assert_eq!(block_counts(&canonical_in_memory_state), (1, 1));
        assert!(canonical_in_memory_state.state_by_hash(block1.recovered_block().hash()).is_none());
        assert!(canonical_in_memory_state
            .header_by_hash(block1.recovered_block().hash())
            .is_none());
        assert!(canonical_in_memory_state
            .in_memory_state()
            .state_by_hash(block1.recovered_block().hash())
            .is_some());
    }

    #[test]
    fn test_in_memory_state_insert_executed_sets_pending_block() {
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let mut test_block_builder = TestBlockBuilder::eth();

        // First random block
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());

        // Second block with parent hash of the first block
        let block2 =
            test_block_builder.get_executed_block_with_number(1, block1.recovered_block().hash());

        // Commit the two blocks
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone(), block2.clone()] };
        canonical_in_memory_state_writer.update_chain(chain);

        // Assert that the pending state is None before setting it
        assert!(canonical_in_memory_state.pending_state().is_none());

        // Insert a block on top of the canonical head, which makes it the pending block.
        let block3 =
            test_block_builder.get_executed_block_with_number(2, block2.recovered_block().hash());
        assert!(canonical_in_memory_state_writer.insert_executed(block3.clone()));

        // Check the pending state
        assert_eq!(
            *canonical_in_memory_state.pending_state().unwrap(),
            BlockState::with_parent(
                block3.clone(),
                Some(Arc::new(BlockState::with_parent(
                    block2,
                    Some(Arc::new(BlockState::new(block1)))
                )))
            )
        );
        // The pending block is tracked but not canonical.
        assert_eq!(
            canonical_in_memory_state
                .in_memory_state()
                .state_by_hash(block3.recovered_block().hash())
                .map(|state| state.hash()),
            Some(block3.recovered_block().hash())
        );
        assert!(canonical_in_memory_state.state_by_hash(block3.recovered_block().hash()).is_none());

        // Check the pending block
        assert_eq!(
            canonical_in_memory_state.pending_block().unwrap(),
            block3.recovered_block().sealed_block().clone()
        );

        // Check the pending block number and hash
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 2, hash: block3.recovered_block().hash() }
        );

        // Check the pending header
        assert_eq!(
            canonical_in_memory_state.pending_header().unwrap(),
            block3.recovered_block().header().clone()
        );

        // Check the pending sealed header
        assert_eq!(
            canonical_in_memory_state.pending_sealed_header().unwrap(),
            block3.recovered_block().clone_sealed_header()
        );

        // Check the pending block with senders
        let pending_block = canonical_in_memory_state.pending_recovered_block().unwrap();
        assert!(Arc::ptr_eq(&pending_block, &block3.recovered_block));

        // Check the pending block and receipts
        let pending = canonical_in_memory_state.pending_block_and_receipts().unwrap();
        assert!(Arc::ptr_eq(pending.block(), &block3.recovered_block));
        assert!(Arc::ptr_eq(pending.execution_output(), &block3.execution_output));
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_empty() {
        let (canonical_in_memory_state, _canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        assert!(canonical_in_memory_state.canonical_chain().next().is_none());
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_single_block() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let hash = block.recovered_block().hash();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![block] });

        let chain = canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();

        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].number(), 1);
        assert_eq!(chain[0].hash(), hash);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_multiple_blocks() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();

        for i in 1..=3 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            canonical_in_memory_state_writer
                .update_chain(NewCanonicalChain::Commit { new: vec![block] });
            parent_hash = hash;
        }

        let chain = canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();

        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].number(), 3);
        assert_eq!(chain[1].number(), 2);
        assert_eq!(chain[2].number(), 1);
    }

    // ensures the pending block is not part of the canonical chain
    #[test]
    fn test_canonical_in_memory_state_canonical_chain_with_pending_block() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::<EthPrimitives>::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();

        for i in 1..=2 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            canonical_in_memory_state_writer
                .update_chain(NewCanonicalChain::Commit { new: vec![block] });
            parent_hash = hash;
        }

        let pending_block = block_builder.get_executed_block_with_number(3, parent_hash);
        canonical_in_memory_state_writer.insert_executed(pending_block);
        assert!(canonical_in_memory_state.pending_state().is_some());
        let chain = canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();

        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].number(), 2);
        assert_eq!(chain[1].number(), 1);
    }

    #[test]
    fn only_a_block_built_on_the_canonical_head_becomes_pending() {
        let mut builder = TestBlockBuilder::eth();
        let persisted = builder.get_executed_block_with_number(1, B256::random());
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state_with_head(persisted.recovered_block().clone_sealed_header());

        // The canonical head can be a block that is not tracked.
        let block2 = builder.get_executed_block_with_number(2, hash(&persisted));
        assert!(canonical_in_memory_state_writer.insert_executed(block2.clone()));
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(block2.recovered_block().num_hash())
        );

        // A block built on another block does not replace the pending block.
        let block3 = builder.get_executed_block_with_number(3, hash(&block2));
        let fork = builder.get_executed_block_with_number(2, B256::random());
        assert!(canonical_in_memory_state_writer.insert_executed(block3.clone()));
        assert!(canonical_in_memory_state_writer.insert_executed(fork));
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(block2.recovered_block().num_hash())
        );

        // Another block built on the canonical head does.
        let sibling = builder.get_executed_block_with_number(2, hash(&persisted));
        assert!(canonical_in_memory_state_writer.insert_executed(sibling.clone()));
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(sibling.recovered_block().num_hash())
        );

        // Once block 2 is the canonical head, block 3 is built on it. It is tracked already and
        // becomes the pending block when it is executed again.
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![block2] });
        assert!(canonical_in_memory_state.pending_state().is_none());
        assert!(!canonical_in_memory_state_writer.insert_executed(block3.clone()));
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(block3.recovered_block().num_hash())
        );
        assert_eq!(block_counts(&canonical_in_memory_state), (1, 3));
        assert_invariants(&canonical_in_memory_state);
    }

    #[test]
    fn operations_that_change_the_canonical_head_set_it_and_clear_the_pending_block() {
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..5).collect::<Vec<_>>();
        let persisted_head = builder
            .get_executed_block_with_number(0, B256::random())
            .recovered_block()
            .clone_sealed_header();

        // Tracks blocks 1 to 3 as canonical and a pending block on top of them.
        let setup = |builder: &mut TestBlockBuilder| {
            let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
                canonical_in_memory_state();
            canonical_in_memory_state_writer
                .update_chain(NewCanonicalChain::Commit { new: blocks[..3].to_vec() });
            assert_eq!(
                canonical_in_memory_state.get_canonical_head(),
                blocks[2].recovered_block().clone_sealed_header()
            );
            canonical_in_memory_state_writer
                .insert_executed(builder.get_executed_block_with_number(4, hash(&blocks[2])));
            assert!(canonical_in_memory_state.pending_state().is_some());
            (canonical_in_memory_state, canonical_in_memory_state_writer)
        };

        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) = setup(&mut builder);
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![blocks[3].clone()] });
        assert_eq!(
            canonical_in_memory_state.get_canonical_head(),
            blocks[3].recovered_block().clone_sealed_header()
        );
        assert_eq!(
            canonical_in_memory_state.get_canonical_num_hash(),
            blocks[3].recovered_block().num_hash()
        );
        assert_eq!(canonical_in_memory_state.get_canonical_block_number(), 4);
        assert_eq!(canonical_in_memory_state.chain_info().best_hash, hash(&blocks[3]));
        assert_eq!(canonical_in_memory_state.head_state().unwrap().hash(), hash(&blocks[3]));
        assert!(canonical_in_memory_state.pending_state().is_none());

        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) = setup(&mut builder);
        canonical_in_memory_state_writer.unwind_canonical_head(blocks[1].clone());
        assert_eq!(
            canonical_in_memory_state.get_canonical_head(),
            blocks[1].recovered_block().clone_sealed_header()
        );
        assert_eq!(canonical_in_memory_state.head_state().unwrap().hash(), hash(&blocks[1]));
        assert!(canonical_in_memory_state.pending_state().is_none());

        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) = setup(&mut builder);
        canonical_in_memory_state_writer.demote_canonical_chain(persisted_head.clone());
        assert_eq!(canonical_in_memory_state.get_canonical_head(), persisted_head);
        assert!(canonical_in_memory_state.head_state().is_none());
        assert_eq!(block_counts(&canonical_in_memory_state), (0, 4));
        assert!(canonical_in_memory_state.pending_state().is_none());

        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) = setup(&mut builder);
        assert_eq!(block_counts(&canonical_in_memory_state), (3, 1));
        canonical_in_memory_state_writer.clear(persisted_head.clone());
        assert_eq!(canonical_in_memory_state.get_canonical_head(), persisted_head);
        assert!(canonical_in_memory_state.head_state().is_none());
        assert_eq!(block_counts(&canonical_in_memory_state), (0, 0));
        assert!(canonical_in_memory_state.pending_state().is_none());
    }

    #[test]
    fn removals_keep_the_chain_info_and_clear_a_removed_pending_block() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let blocks = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let pending = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        canonical_in_memory_state_writer.insert_executed(pending.clone());
        let canonical_head = canonical_in_memory_state.get_canonical_head();

        // The pending block is built on a block that stays tracked.
        let persisted = blocks[1].recovered_block().num_hash();
        assert_eq!(
            canonical_in_memory_state_writer
                .remove_canonical_until(persisted.hash, persisted.number),
            InMemoryCanonicalBlocksRemoval::Removed(2)
        );
        assert_eq!(canonical_in_memory_state_writer.prune_conflicting_with_finalized(persisted), 0);
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(pending.recovered_block().num_hash())
        );
        assert_eq!(canonical_in_memory_state.pending_state().unwrap().chain().count(), 2);
        assert_eq!(canonical_in_memory_state.get_canonical_head(), canonical_head);

        // A block at the height of the pending block is finalized, which removes the pending
        // block.
        let finalized = BlockNumHash::new(4, B256::random());
        assert_eq!(canonical_in_memory_state_writer.prune_conflicting_with_finalized(finalized), 1);
        assert!(canonical_in_memory_state.pending_state().is_none());
        assert_eq!(canonical_in_memory_state.get_canonical_head(), canonical_head);
        assert_invariants(&canonical_in_memory_state);
    }

    #[test]
    fn unwind_canonical_head_to_an_in_memory_ancestor() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        canonical_in_memory_state_writer.unwind_canonical_head(blocks[2].clone());

        assert_eq!(
            canonical_in_memory_state.get_canonical_num_hash(),
            blocks[2].recovered_block().num_hash()
        );
        assert_eq!(
            canonical_in_memory_state
                .canonical_chain()
                .map(|state| state.hash())
                .collect::<Vec<_>>(),
            vec![hash(&blocks[2]), hash(&blocks[1]), hash(&blocks[0])]
        );
        assert_eq!(block_counts(&canonical_in_memory_state), (3, 2));
        // The blocks above the new head stay tracked on top of it.
        assert!(canonical_in_memory_state.state_by_hash(hash(&blocks[4])).is_none());
        let old_head =
            canonical_in_memory_state.in_memory_state().state_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(old_head.chain().count(), 5);

        // The block built on the new head is tracked. Executing it again makes it pending.
        assert!(!canonical_in_memory_state_writer.insert_executed(blocks[3].clone()));
        assert_eq!(
            canonical_in_memory_state.pending_block_num_hash(),
            Some(blocks[3].recovered_block().num_hash())
        );
        assert_invariants(&canonical_in_memory_state);
    }

    #[test]
    fn unwind_canonical_head_to_a_persisted_ancestor() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        // Blocks 1 to 3 are persisted and not tracked, 4 and 5 are the in-memory canonical chain.
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[3..].to_vec() });

        canonical_in_memory_state_writer.unwind_canonical_head(blocks[1].clone());

        assert_eq!(
            canonical_in_memory_state.get_canonical_num_hash(),
            blocks[1].recovered_block().num_hash()
        );
        // The new head is the only canonical in-memory block.
        assert_eq!(
            canonical_in_memory_state
                .canonical_chain()
                .map(|state| state.hash())
                .collect::<Vec<_>>(),
            vec![hash(&blocks[1])]
        );
        assert_eq!(block_counts(&canonical_in_memory_state), (1, 2));
        let old_head =
            canonical_in_memory_state.in_memory_state().state_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(old_head.chain().count(), 2);
        assert_eq!(old_head.anchor(), blocks[2].recovered_block().num_hash());
        assert_invariants(&canonical_in_memory_state);
    }

    #[test]
    #[should_panic(expected = "the writer does not write to the in-memory state")]
    fn writer_of_another_in_memory_state_is_rejected() {
        let (canonical_in_memory_state, _canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        let (_, other_in_memory_state_writer) = InMemoryState::new();

        let _ = CanonicalInMemoryStateWriter::new(
            canonical_in_memory_state,
            other_in_memory_state_writer,
        );
    }

    #[test]
    #[should_panic(
        expected = "the highest canonical in-memory block is not the canonical head of the chain info"
    )]
    fn canonical_head_that_is_not_the_highest_canonical_block_is_rejected() {
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..3).collect::<Vec<_>>();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: blocks[0].recovered_block().clone_sealed_header(),
                safe: None,
                finalized: None,
            },
            in_memory_state,
        );

        let _ =
            CanonicalInMemoryStateWriter::new(canonical_in_memory_state, in_memory_state_writer);
    }

    #[test]
    fn canonical_head_that_is_the_highest_canonical_block_is_accepted() {
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..3).collect::<Vec<_>>();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: blocks[1].recovered_block().clone_sealed_header(),
                safe: None,
                finalized: None,
            },
            in_memory_state,
        );

        let canonical_in_memory_state_writer =
            CanonicalInMemoryStateWriter::new(canonical_in_memory_state, in_memory_state_writer);
        assert_eq!(
            canonical_in_memory_state_writer
                .canonical_in_memory_state()
                .head_state()
                .unwrap()
                .hash(),
            hash(&blocks[1])
        );
    }

    #[test]
    fn writer_sets_the_chain_info_and_notifies_subscribers() {
        let mut builder = TestBlockBuilder::eth();
        let (canonical_in_memory_state, mut canonical_in_memory_state_writer) =
            canonical_in_memory_state();
        assert_eq!(
            canonical_in_memory_state_writer.canonical_in_memory_state().get_canonical_head(),
            SealedHeader::default()
        );
        let block = builder.get_executed_block_with_number(1, B256::random());
        let header = block.recovered_block().clone_sealed_header();

        assert!(canonical_in_memory_state.get_safe_header().is_none());
        canonical_in_memory_state_writer.set_safe(header.clone());
        assert_eq!(canonical_in_memory_state.get_safe_header(), Some(header.clone()));
        assert_eq!(canonical_in_memory_state.get_safe_num_hash(), Some(header.num_hash()));

        assert!(canonical_in_memory_state.get_finalized_header().is_none());
        canonical_in_memory_state_writer.set_finalized(header.clone());
        assert_eq!(canonical_in_memory_state.get_finalized_header(), Some(header.clone()));
        assert_eq!(canonical_in_memory_state.get_finalized_num_hash(), Some(header.num_hash()));

        assert!(canonical_in_memory_state.get_persisted_num_hash().is_none());
        canonical_in_memory_state_writer.set_persisted(header.num_hash());
        assert_eq!(canonical_in_memory_state.get_persisted_num_hash(), Some(header.num_hash()));

        assert!(canonical_in_memory_state.last_received_update_timestamp().is_none());
        canonical_in_memory_state_writer.on_forkchoice_update_received();
        assert!(canonical_in_memory_state.last_received_update_timestamp().is_some());

        let mut notifications = canonical_in_memory_state.subscribe_canon_state();
        let notification = NewCanonicalChain::Commit { new: vec![block] }.to_chain_notification();
        canonical_in_memory_state_writer.notify_canon_state(notification.clone());
        assert_eq!(notifications.try_recv().unwrap(), notification);
    }
}
