use super::{
    blocks::{InMemoryBlocks, InMemoryBlocksStats},
    BlockState, ExecutedBlock, NewCanonicalChain,
};
use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications,
    ChainInfoHeaders, ChainInfoTracker,
};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Map, BlockNumber, TxHash, B256};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::RecoveredBlockAndExecutionOutput;
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives_traits::{
    BlockBody as _, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::{broadcast, watch};

/// Size of the broadcast channel used to notify canonical state events.
const CANON_STATE_NOTIFICATION_CHANNEL_SIZE: usize = 256;

/// Metrics for the in-memory state.
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.in_mem_state")]
pub(crate) struct InMemoryStateMetrics {
    /// The block number of the earliest block in the in-memory state.
    pub(crate) earliest_block: Gauge,
    /// The block number of the latest block in the in-memory state.
    pub(crate) latest_block: Gauge,
    /// The number of blocks in the in-memory state.
    pub(crate) num_blocks: Gauge,
}

/// Container type for in memory state data of the executed blocks that are not on disk yet.
///
/// This tracks blocks and their state that haven't been persisted to disk yet: the blocks that
/// are part of the canonical chain that can be traced back to a canonical block on disk, and the
/// executed blocks that are not canonical.
///
/// A node has one in-memory state. Clones are handles to the same blocks, pending block and
/// metrics, and the default value is an empty state of its own.
///
/// # Locking behavior on state updates
///
/// Updates change the blocks, the numbers and the pending block under one write lock. This
/// prevents readers from observing partially updated state where they are out of sync with each
/// other, or a block that is both canonical and not canonical, or neither.
#[derive(Debug, Clone, Default)]
pub struct InMemoryState<N: NodePrimitives = EthPrimitives> {
    inner: Arc<InMemoryStateInner<N>>,
}

impl<N: NodePrimitives> InMemoryState<N> {
    pub(crate) fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
    ) -> Self {
        let this = Self {
            inner: Arc::new(InMemoryStateInner {
                blocks: RwLock::new(InMemoryBlocks::new(blocks, numbers, pending)),
                removal_generation: Default::default(),
                metrics: Default::default(),
            }),
        };
        this.update_metrics();
        this
    }

    /// Update the metrics for the in-memory state.
    ///
    /// # Locking behavior
    ///
    /// This tries to acquire a read lock. Drop any write locks before calling this.
    pub(crate) fn update_metrics(&self) {
        let stats = self.inner.blocks.read().stats();
        self.record_metrics(stats);
    }

    /// Sets the in-memory state metrics from `stats`.
    fn record_metrics(&self, stats: InMemoryBlocksStats) {
        let InMemoryBlocksStats { count, earliest, latest } = stats;
        if let Some(earliest_block_number) = earliest {
            self.inner.metrics.earliest_block.set(earliest_block_number as f64);
        }
        if let Some(latest_block_number) = latest {
            self.inner.metrics.latest_block.set(latest_block_number as f64);
        }
        self.inner.metrics.num_blocks.set(count as f64);
    }

    /// Returns the state of the tracked block with the given hash, canonical or not.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.blocks.read().get(&hash).cloned()
    }

    /// Returns `true` if a block with the given hash is tracked, canonical or not.
    pub fn contains(&self, hash: &B256) -> bool {
        self.inner.blocks.read().contains(hash)
    }

    /// Returns the numbers of tracked canonical blocks and of tracked blocks that are not
    /// canonical, both taken from the same snapshot of the blocks.
    pub fn block_counts(&self) -> InMemoryStateBlockCounts {
        self.inner.blocks.read().counts()
    }

    /// Returns the state for a given block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.inner.blocks.read().canonical().by_number(number).cloned()
    }

    /// Returns the hash for a specific block number
    pub(crate) fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.inner.blocks.read().canonical().hash_by_number(number)
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.blocks.read().canonical().head().cloned()
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.blocks.read().pending_state().cloned()
    }

    /// Returns a counter that increases every time blocks are removed.
    ///
    /// State derived from the tracked blocks compares it with the value it saw last to learn that
    /// blocks it refers to may no longer be tracked.
    pub fn removal_generation(&self) -> u64 {
        self.inner.removal_generation.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn block_count(&self) -> usize {
        self.inner.blocks.read().canonical().len()
    }

    /// Applies `f` to the blocks under the write lock, bumps the removal generation if `f`
    /// removed blocks, and updates the metrics from the stats read under the same lock.
    ///
    /// Returns what `f` returns, so that the blocks it removed are dropped after the lock is
    /// released.
    fn update<R>(&self, f: impl FnOnce(&mut InMemoryBlocks<N>) -> R) -> R {
        let (result, stats) = {
            let mut blocks = self.inner.blocks.write();
            let removed_count = blocks.removed_count();
            let result = f(&mut blocks);
            #[cfg(debug_assertions)]
            blocks.assert_invariants();
            if blocks.removed_count() != removed_count {
                self.inner.removal_generation.fetch_add(1, Ordering::Relaxed);
            }
            (result, blocks.stats())
        };
        self.record_metrics(stats);
        result
    }
}

/// The blocks of an [`InMemoryState`] with their lock, and what is derived from them.
#[derive(Debug, Default)]
struct InMemoryStateInner<N: NodePrimitives> {
    /// The executed blocks that are not on disk yet, canonical or not.
    blocks: RwLock<InMemoryBlocks<N>>,
    /// Counts the updates that removed blocks.
    removal_generation: AtomicU64,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

/// The numbers of blocks an [`InMemoryState`] tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InMemoryStateBlockCounts {
    /// The number of canonical blocks.
    pub canonical: usize,
    /// The number of executed blocks that are not canonical, including the pending block.
    pub non_canonical: usize,
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner<N: NodePrimitives> {
    /// Tracks certain chain information, such as the canonical head, safe head, and finalized
    /// head.
    pub(crate) chain_info_tracker: ChainInfoTracker<N>,
    /// Tracks blocks at the tip of the chain that have not been persisted to disk yet.
    pub(crate) in_memory_state: InMemoryState<N>,
    /// A broadcast stream that emits events when the canonical chain is updated.
    pub(crate) canon_state_notification_sender: CanonStateNotificationSender<N>,
}

impl<N: NodePrimitives> CanonicalInMemoryStateInner<N> {
    /// Clears all entries in the in memory state.
    fn clear(&self) {
        self.in_memory_state.update(InMemoryBlocks::clear);
    }
}

/// This type is responsible for providing the blocks, receipts, and state for
/// all canonical blocks not on disk yet and keeps track of the block range that
/// is in memory.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState<N: NodePrimitives = EthPrimitives> {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner<N>>,
}

impl<N: NodePrimitives> CanonicalInMemoryState<N> {
    /// Create a new in-memory state with the given blocks, numbers, pending state, and optional
    /// finalized header.
    pub fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let in_memory_state = InMemoryState::new(blocks, numbers, pending);
        let header = in_memory_state.head_state().map_or_else(SealedHeader::default, |state| {
            state.block_ref().recovered_block().clone_sealed_header()
        });
        let chain_info_tracker =
            ChainInfoTracker::new(ChainInfoHeaders { canonical_head: header, safe, finalized });
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);

        Self {
            inner: Arc::new(CanonicalInMemoryStateInner {
                chain_info_tracker,
                in_memory_state,
                canon_state_notification_sender,
            }),
        }
    }

    /// Create an empty state.
    pub fn empty() -> Self {
        Self::new(B256Map::default(), BTreeMap::new(), None, None, None)
    }

    /// Creates a canonical state over the blocks of `in_memory_state`, with the canonical head
    /// and the safe and finalized blocks of `chain_info_headers`.
    ///
    /// The blocks are shared with every other handle of `in_memory_state`, so an update of the
    /// blocks through the returned value is visible through all of them. The chain info and the
    /// notifications belong to the returned value and its clones.
    pub fn with_head(
        chain_info_headers: ChainInfoHeaders<N::BlockHeader>,
        in_memory_state: InMemoryState<N>,
    ) -> Self {
        let chain_info_tracker = ChainInfoTracker::new(chain_info_headers);
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);
        let inner = CanonicalInMemoryStateInner {
            chain_info_tracker,
            in_memory_state,
            canon_state_notification_sender,
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns the block hash corresponding to the given number.
    pub fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.inner.in_memory_state.hash_by_number(number)
    }

    /// Returns the header corresponding to the given hash.
    pub fn header_by_hash(&self, hash: B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.state_by_hash(hash)
            .map(|block| block.block_ref().recovered_block().clone_sealed_header())
    }

    /// Clears all entries in the in memory state.
    pub fn clear_state(&self) {
        self.inner.clear()
    }

    /// Tracks an executed block that is not canonical, unless it is already tracked.
    ///
    /// The block is connected to its parent block, canonical or not. It is not returned by the
    /// canonical lookups until [`Self::update_chain`] makes it canonical. Does not change the
    /// pending block.
    pub fn insert_executed(&self, block: ExecutedBlock<N>) {
        self.inner.in_memory_state.update(|blocks| blocks.insert_executed(block));
    }

    /// Updates the pending block with the given block, and tracks it like
    /// [`Self::insert_executed`].
    ///
    /// Note: This assumes that the parent block of the pending block is canonical.
    pub fn set_pending_block(&self, pending: ExecutedBlock<N>) {
        self.inner.in_memory_state.update(|blocks| blocks.insert_pending(pending));
    }

    /// Update the in memory state with the given chain update.
    ///
    /// This appends the new blocks to the tracked chain and connects them to their parent blocks.
    /// The blocks a reorg replaced stay tracked as executed blocks that are not canonical, and so
    /// does the pending block, which is no longer pending.
    pub fn update_chain(&self, new_chain: NewCanonicalChain<N>) {
        self.inner.in_memory_state.update(|blocks| blocks.update_chain(new_chain));
    }

    /// Applies a backfill sync that finished at `backfill` without unwinding, in one update.
    ///
    /// Removes the canonical blocks up to and including its height like
    /// [`Self::remove_canonical_until`], makes every remaining canonical block an executed block
    /// that is not canonical, and removes the executed blocks that conflict with `backfill` like
    /// [`Self::prune_conflicting_with_finalized`]. Afterwards there is no pending block.
    ///
    /// If the canonical block at the height of `backfill` is another block, the canonical blocks
    /// are only removed as conflicting blocks, together with the blocks built on that block.
    pub fn on_backfill_finished(&self, persisted_hash: B256, backfill: BlockNumHash) {
        self.inner.in_memory_state.update(|blocks| {
            let removed = if blocks
                .canonical()
                .hash_by_number(backfill.number)
                .is_none_or(|hash| hash == backfill.hash)
            {
                blocks.remove_canonical_until(persisted_hash, backfill.number)
            } else {
                Vec::new()
            };
            blocks.demote_canonical_chain();
            (removed, blocks.prune_conflicting_with_finalized(backfill))
        });
    }

    /// Applies the persistence of blocks.
    ///
    /// Reports `last_persisted_block` as the persisted tip, and then removes blocks in one update:
    /// the canonical blocks up to and including `state_trie_persisted_block` like
    /// [`Self::remove_persisted_blocks_until`].
    /// It also removes the executed blocks that are not canonical and whose chain builds on a
    /// block above `last_persisted_block`: that block was removed from disk and is not tracked, so
    /// they are not connected to the persisted chain anymore.
    /// If there is a `finalized` block, it also removes the executed blocks that conflict with it
    /// like [`Self::prune_conflicting_with_finalized`]. A finalized block above
    /// `state_trie_persisted_block` is replaced by that block, so that only the blocks that
    /// conflict with the persisted chain are removed.
    pub fn on_blocks_persisted(
        &self,
        last_persisted_block: BlockNumHash,
        state_trie_persisted_block: BlockNumHash,
        finalized: Option<BlockNumHash>,
    ) {
        self.set_persisted(last_persisted_block);
        self.inner.in_memory_state.update(|blocks| {
            let removed = blocks.remove_canonical_until(
                last_persisted_block.hash,
                state_trie_persisted_block.number.min(last_persisted_block.number),
            );
            let disconnected = blocks.prune_built_above(last_persisted_block.number);
            let pruned = finalized.map(|finalized| {
                blocks.prune_conflicting_with_finalized(
                    if state_trie_persisted_block.number < finalized.number {
                        state_trie_persisted_block
                    } else {
                        finalized
                    },
                )
            });
            (removed, disconnected, pruned)
        });
    }

    /// Removes the executed blocks that are not canonical and can no longer become canonical once
    /// `finalized` is final: the ones below its height, the other ones at its height, and every
    /// block built on such a block at its height.
    pub fn prune_conflicting_with_finalized(&self, finalized: BlockNumHash) {
        self.inner
            .in_memory_state
            .update(|blocks| blocks.prune_conflicting_with_finalized(finalized));
    }

    /// Removes blocks from the in memory state that are persisted to the given height.
    ///
    /// This will update the links between blocks and remove all blocks that are [..
    /// `persisted_height`].
    pub fn remove_persisted_blocks(&self, persisted_num_hash: BlockNumHash) {
        self.remove_persisted_blocks_until(persisted_num_hash, persisted_num_hash.number);
    }

    /// Removes blocks from the in-memory state through `remove_until` while still reporting the
    /// provided block as the persisted tip.
    ///
    /// Blocks above the persisted block are never removed, and the pending block is kept. If the
    /// persisted block is neither a canonical in-memory block nor the block the in-memory
    /// canonical chain builds on, it is still reported as the persisted tip, but no blocks are
    /// removed.
    pub fn remove_persisted_blocks_until(
        &self,
        persisted_num_hash: BlockNumHash,
        remove_until: BlockNumber,
    ) {
        self.set_persisted(persisted_num_hash);
        self.remove_canonical_until(
            persisted_num_hash.hash,
            remove_until.min(persisted_num_hash.number),
        );
    }

    /// Removes the canonical blocks up to and including `remove_until` if `persisted_hash` is a
    /// canonical in-memory block or the block the in-memory canonical chain builds on.
    ///
    /// Otherwise the canonical blocks in memory are not the ones that were persisted, and nothing
    /// is removed. The chain info is unchanged.
    pub fn remove_canonical_until(&self, persisted_hash: B256, remove_until: BlockNumber) {
        self.inner
            .in_memory_state
            .update(|blocks| blocks.remove_canonical_until(persisted_hash, remove_until));
    }

    /// Returns in memory state corresponding the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.inner.blocks.read().canonical().get(&hash).cloned()
    }

    /// Returns in memory state corresponding the block number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    /// Returns the in memory head state.
    pub fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state()
    }

    /// Returns the in memory pending state.
    pub fn pending_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.pending_state()
    }

    /// Returns the in memory pending `BlockNumHash`.
    pub fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        self.inner
            .in_memory_state
            .pending_state()
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

    /// Hook for new fork choice update.
    pub fn on_forkchoice_update_received(&self) {
        self.inner.chain_info_tracker.on_forkchoice_update_received();
    }

    /// Returns the timestamp of the last received update.
    pub fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_forkchoice_update_received_at()
    }

    /// Canonical head setter.
    pub fn set_canonical_head(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_canonical_head(header);
    }

    /// Safe head setter.
    pub fn set_safe(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_safe(header);
    }

    /// Finalized head setter.
    pub fn set_finalized(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_finalized(header);
    }

    /// Persisted block setter.
    pub fn set_persisted(&self, num_hash: BlockNumHash) {
        self.inner.chain_info_tracker.set_persisted(num_hash);
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

    /// Attempts to send a new [`CanonStateNotification`] to all active Receiver handles.
    pub fn notify_canon_state(&self, event: CanonStateNotification<N>) {
        self.inner.canon_state_notification_sender.send(event).ok();
    }

    /// Returns an iterator over all __canonical blocks__ in the in-memory state, from newest to
    /// oldest (highest to lowest).
    ///
    /// This iterator contains a snapshot of the in-memory state at the time of the call.
    pub fn canonical_chain(&self) -> impl Iterator<Item = Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state().into_iter().flat_map(|head| head.iter())
    }

    /// Returns [`SignedTransaction`] type for the given `TxHash` if found.
    ///
    /// [`SignedTransaction`]: reth_primitives_traits::SignedTransaction
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

    /// Returns a tuple with [`SignedTransaction`] type and [`TransactionMeta`] for the
    /// given [`TxHash`] if found.
    ///
    /// [`SignedTransaction`]: reth_primitives_traits::SignedTransaction
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
}

/// Creates a [`CanonicalInMemoryState`] and changes it: the blocks it tracks, its chain info, and
/// the notifications it sends.
///
/// A writer is created together with its state. [`Self::canonical_in_memory_state`] returns that
/// state, which can be cloned and is what everything but the owner of the writer reads. What is
/// written through the writer is read through the state and every clone of it.
///
/// A writer cannot be cloned and its methods take `&mut self`, so only its owner changes the
/// state through it. The constructors of [`CanonicalInMemoryState`] and its methods that change
/// it are still public.
#[derive(Debug)]
pub struct CanonicalInMemoryStateWriter<N: NodePrimitives = EthPrimitives> {
    canonical_in_memory_state: CanonicalInMemoryState<N>,
}

impl<N: NodePrimitives> CanonicalInMemoryStateWriter<N> {
    /// Creates a canonical state over the blocks of `in_memory_state`, with the canonical head
    /// and the safe and finalized blocks of `chain_info_headers`, and returns its writer.
    ///
    /// The blocks are shared with every other handle of `in_memory_state`. The chain info and the
    /// notifications belong to the created state and its clones.
    pub fn new(
        chain_info_headers: ChainInfoHeaders<N::BlockHeader>,
        in_memory_state: InMemoryState<N>,
    ) -> Self {
        Self {
            canonical_in_memory_state: CanonicalInMemoryState::with_head(
                chain_info_headers,
                in_memory_state,
            ),
        }
    }

    /// Returns the state this changes.
    pub const fn canonical_in_memory_state(&self) -> &CanonicalInMemoryState<N> {
        &self.canonical_in_memory_state
    }

    /// See [`CanonicalInMemoryState::clear_state`].
    pub fn clear_state(&mut self) {
        self.canonical_in_memory_state.clear_state();
    }

    /// See [`CanonicalInMemoryState::insert_executed`].
    pub fn insert_executed(&mut self, block: ExecutedBlock<N>) {
        self.canonical_in_memory_state.insert_executed(block);
    }

    /// See [`CanonicalInMemoryState::set_pending_block`].
    pub fn set_pending_block(&mut self, pending: ExecutedBlock<N>) {
        self.canonical_in_memory_state.set_pending_block(pending);
    }

    /// See [`CanonicalInMemoryState::update_chain`].
    pub fn update_chain(&mut self, new_chain: NewCanonicalChain<N>) {
        self.canonical_in_memory_state.update_chain(new_chain);
    }

    /// See [`CanonicalInMemoryState::on_backfill_finished`].
    pub fn on_backfill_finished(
        &mut self,
        last_persisted_hash: B256,
        backfill_block: BlockNumHash,
    ) {
        self.canonical_in_memory_state.on_backfill_finished(last_persisted_hash, backfill_block);
    }

    /// See [`CanonicalInMemoryState::on_blocks_persisted`].
    pub fn on_blocks_persisted(
        &mut self,
        last_persisted_block: BlockNumHash,
        state_trie_persisted_block: BlockNumHash,
        finalized: Option<BlockNumHash>,
    ) {
        self.canonical_in_memory_state.on_blocks_persisted(
            last_persisted_block,
            state_trie_persisted_block,
            finalized,
        );
    }

    /// See [`CanonicalInMemoryState::on_forkchoice_update_received`].
    pub fn on_forkchoice_update_received(&mut self) {
        self.canonical_in_memory_state.on_forkchoice_update_received();
    }

    /// See [`CanonicalInMemoryState::set_canonical_head`].
    pub fn set_canonical_head(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state.set_canonical_head(header);
    }

    /// See [`CanonicalInMemoryState::set_safe`].
    pub fn set_safe(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state.set_safe(header);
    }

    /// See [`CanonicalInMemoryState::set_finalized`].
    pub fn set_finalized(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state.set_finalized(header);
    }

    /// See [`CanonicalInMemoryState::notify_canon_state`].
    pub fn notify_canon_state(&mut self, event: CanonStateNotification<N>) {
        self.canonical_in_memory_state.notify_canon_state(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use rand::Rng;

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    #[test]
    fn test_in_memory_state_impl_state_by_hash() {
        let mut state_by_hash = B256Map::default();
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        state_by_hash.insert(state.hash(), state.clone());

        let in_memory_state = InMemoryState::new(state_by_hash, BTreeMap::new(), None);

        assert_eq!(in_memory_state.state_by_hash(state.hash()), Some(state));
        assert_eq!(in_memory_state.state_by_hash(B256::random()), None);
    }

    #[test]
    fn test_in_memory_state_impl_state_by_number() {
        let mut state_by_hash = B256Map::default();
        let mut hash_by_number = BTreeMap::new();

        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        let hash = state.hash();

        state_by_hash.insert(hash, state.clone());
        hash_by_number.insert(number, hash);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);

        assert_eq!(in_memory_state.state_by_number(number), Some(state));
        assert_eq!(in_memory_state.state_by_number(number + 1), None);
    }

    #[test]
    fn test_in_memory_state_impl_head_state() {
        let mut state_by_hash = B256Map::default();
        let mut hash_by_number = BTreeMap::new();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state1 = Arc::new(create_mock_state(&mut test_block_builder, 1, B256::random()));
        let hash1 = state1.hash();
        let state2 = Arc::new(create_mock_state(&mut test_block_builder, 2, hash1));
        let hash2 = state2.hash();
        hash_by_number.insert(1, hash1);
        hash_by_number.insert(2, hash2);
        state_by_hash.insert(hash1, state1);
        state_by_hash.insert(hash2, state2);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);
        let head_state = in_memory_state.head_state().unwrap();

        assert_eq!(head_state.hash(), hash2);
        assert_eq!(head_state.number(), 2);
    }

    #[test]
    fn test_in_memory_state_impl_pending_state() {
        let pending_number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let pending_state =
            create_mock_state(&mut test_block_builder, pending_number, B256::random());
        let pending_hash = pending_state.hash();

        let in_memory_state =
            InMemoryState::new(B256Map::default(), BTreeMap::new(), Some(pending_state));

        let result = in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.block.recovered_block().hash(), pending_hash);
        assert_eq!(actual_pending_state.block.recovered_block().number, pending_number);
    }

    #[test]
    fn test_in_memory_state_impl_no_pending_state() {
        let in_memory_state: InMemoryState =
            InMemoryState::new(B256Map::default(), BTreeMap::new(), None);

        assert_eq!(in_memory_state.pending_state(), None);
    }

    #[test]
    fn test_in_memory_state_chain_update() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone()] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );

        let chain = NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );

        assert_eq!(state.inner.in_memory_state.block_count(), 1);
    }

    #[test]
    fn test_in_memory_state_set_pending_block() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();

        // First random block
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());

        // Second block with parent hash of the first block
        let block2 =
            test_block_builder.get_executed_block_with_number(1, block1.recovered_block().hash());

        // Commit the two blocks
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone(), block2.clone()] };
        state.update_chain(chain);

        // Assert that the pending state is None before setting it
        assert!(state.pending_state().is_none());

        // Set the pending block
        state.set_pending_block(block2.clone());

        // Check the pending state
        assert_eq!(
            *state.pending_state().unwrap(),
            BlockState::with_parent(block2.clone(), Some(Arc::new(BlockState::new(block1))))
        );

        // Check the pending block
        assert_eq!(state.pending_block().unwrap(), block2.recovered_block().sealed_block().clone());

        // Check the pending block number and hash
        assert_eq!(
            state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 1, hash: block2.recovered_block().hash() }
        );

        // Check the pending header
        assert_eq!(state.pending_header().unwrap(), block2.recovered_block().header().clone());

        // Check the pending sealed header
        assert_eq!(
            state.pending_sealed_header().unwrap(),
            block2.recovered_block().clone_sealed_header()
        );

        // Check the pending block with senders
        let pending_block = state.pending_recovered_block().unwrap();
        assert!(Arc::ptr_eq(&pending_block, &block2.recovered_block));

        // Check the pending block and receipts
        let pending = state.pending_block_and_receipts().unwrap();
        assert!(Arc::ptr_eq(pending.block(), &block2.recovered_block));
        assert!(Arc::ptr_eq(pending.execution_output(), &block2.execution_output));
    }

    #[test]
    fn test_in_memory_state_executed_blocks_are_not_canonical() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(1, B256::random());
        let block1_hash = block1.recovered_block().hash();
        let block2 = test_block_builder.get_executed_block_with_number(2, block1_hash);
        let block2_hash = block2.recovered_block().hash();
        let fork2 = test_block_builder.get_executed_block_with_number(2, block1_hash);
        let fork2_hash = fork2.recovered_block().hash();
        let tracked = |hash| state.inner.in_memory_state.inner.blocks.read().contains(&hash);

        // Executed blocks are tracked, but not canonical.
        state.insert_executed(block1.clone());
        state.insert_executed(block2.clone());
        state.insert_executed(fork2.clone());
        assert!(tracked(block1_hash) && tracked(block2_hash) && tracked(fork2_hash));
        assert!(state.state_by_hash(block2_hash).is_none());
        assert!(state.head_state().is_none());
        assert!(state.pending_state().is_none());

        // A chain update makes them canonical.
        state.update_chain(NewCanonicalChain::Commit { new: vec![block1, block2.clone()] });
        assert_eq!(state.head_state().unwrap().hash(), block2_hash);
        assert!(state.state_by_hash(fork2_hash).is_none());

        // A reorg keeps the replaced block tracked.
        state.update_chain(NewCanonicalChain::Reorg { new: vec![fork2], old: vec![block2] });
        assert_eq!(state.head_state().unwrap().hash(), fork2_hash);
        assert!(state.state_by_hash(block2_hash).is_none());
        assert!(tracked(block2_hash));

        // The replaced block conflicts with the block that replaced it once that is finalized.
        state.prune_conflicting_with_finalized(BlockNumHash::new(2, fork2_hash));
        assert!(!tracked(block2_hash));
        assert_eq!(state.inner.in_memory_state.block_count(), 2);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_empty() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        assert!(state.canonical_chain().next().is_none());
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_single_block() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let hash = block.recovered_block().hash();
        let mut blocks = B256Map::default();
        blocks.insert(hash, Arc::new(BlockState::new(block)));
        let mut numbers = BTreeMap::new();
        numbers.insert(1, hash);

        let state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].number(), 1);
        assert_eq!(chain[0].hash(), hash);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_multiple_blocks() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::eth();
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=3 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_chain(NewCanonicalChain::Commit { new: vec![block] });
            parent_hash = hash;
        }

        let chain: Vec<_> = state.canonical_chain().collect();

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
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=2 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_chain(NewCanonicalChain::Commit { new: vec![block] });
            parent_hash = hash;
        }

        let pending_block = block_builder.get_executed_block_with_number(3, parent_hash);
        state.set_pending_block(pending_block);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].number(), 2);
        assert_eq!(chain[1].number(), 1);
    }

    #[test]
    fn test_canonical_in_memory_state_remove_persisted_blocks() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();
        let num_hash = |number: usize| blocks[number - 1].recovered_block().num_hash();
        // The numbers of the blocks a state is linked to, itself included.
        let chain = |state: &BlockState| state.chain().map(BlockState::number).collect::<Vec<_>>();

        state.update_chain(NewCanonicalChain::Commit { new: blocks[..4].to_vec() });
        state.set_pending_block(blocks[4].clone());

        // A persisted block that is not tracked is recorded, but removes nothing.
        let untracked = BlockNumHash::new(2, B256::random());
        state.remove_persisted_blocks(untracked);
        assert_eq!(state.get_persisted_num_hash(), Some(untracked));
        assert_eq!(chain(&state.head_state().unwrap()), [4, 3, 2, 1]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4, 3, 2, 1]);

        // Only the blocks through the given number are removed. The blocks that stay and the
        // pending block are no longer linked to the removed ones.
        state.remove_persisted_blocks_until(num_hash(2), 1);
        assert_eq!(state.get_persisted_num_hash(), Some(num_hash(2)));
        assert_eq!(state.hash_by_number(1), None);
        assert_eq!(state.state_by_hash(num_hash(1).hash), None);
        assert_eq!(chain(&state.state_by_number(2).unwrap()), [2]);
        assert_eq!(chain(&state.head_state().unwrap()), [4, 3, 2]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4, 3, 2]);

        // No block above the persisted block is removed.
        state.remove_persisted_blocks_until(num_hash(3), 4);
        assert_eq!(state.hash_by_number(3), None);
        assert_eq!(chain(&state.head_state().unwrap()), [4]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4]);

        // The pending block stays when the last canonical block is removed.
        state.remove_persisted_blocks(num_hash(4));
        assert_eq!(state.head_state(), None);
        assert_eq!(state.inner.in_memory_state.block_count(), 0);
        assert_eq!(chain(&state.pending_state().unwrap()), [5]);

        state.clear_state();
        assert_eq!(state.pending_state(), None);
    }

    #[test]
    fn test_in_memory_state_removal_generation() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let fork = block_builder.get_executed_block_with_number(2, hash(1));
        let fork_hash = fork.recovered_block().hash();

        // Tracking blocks and making them canonical removes nothing.
        let generation = in_memory_state.removal_generation();
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        state.insert_executed(fork);
        assert_eq!(in_memory_state.removal_generation(), generation);

        // The in-memory state returns a tracked block of either section, the canonical state
        // only a canonical one.
        assert_eq!(in_memory_state.state_by_hash(fork_hash).unwrap().hash(), fork_hash);
        assert_eq!(in_memory_state.state_by_hash(hash(3)).unwrap().hash(), hash(3));
        assert_eq!(state.state_by_hash(fork_hash), None);

        // Nothing is removed for a block that is not on the canonical chain.
        state.remove_canonical_until(fork_hash, 3);
        assert_eq!(in_memory_state.removal_generation(), generation);
        assert_eq!(state.canonical_chain().count(), 3);

        // Canonical blocks are removed for a canonical block.
        state.remove_canonical_until(hash(2), 1);
        let after_canonical_block = in_memory_state.removal_generation();
        assert!(after_canonical_block > generation);
        assert_eq!(in_memory_state.state_by_hash(hash(1)), None);

        // They are also removed for the block the canonical chain builds on.
        state.remove_canonical_until(hash(1), 2);
        let after_chain_base = in_memory_state.removal_generation();
        assert!(after_chain_base > after_canonical_block);
        assert_eq!(state.canonical_chain().map(|state| state.number()).collect::<Vec<_>>(), [3]);

        // Removing blocks that are not canonical counts as well.
        state.prune_conflicting_with_finalized(blocks[2].recovered_block().num_hash());
        let after_fork = in_memory_state.removal_generation();
        assert!(after_fork > after_chain_base);
        assert_eq!(in_memory_state.state_by_hash(fork_hash), None);

        state.clear_state();
        assert!(in_memory_state.removal_generation() > after_fork);
        assert_eq!(in_memory_state.state_by_hash(hash(3)), None);
    }

    #[test]
    fn test_on_backfill_finished_is_one_update() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let fork = block_builder.get_executed_block_with_number(2, hash(1));
        let fork_hash = fork.recovered_block().hash();
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        state.insert_executed(fork);

        // Backfill finishes at block 2: the canonical blocks through it and the fork are removed
        // in one update, and block 3 stays tracked but is no longer canonical.
        let generation = in_memory_state.removal_generation();
        state.on_backfill_finished(hash(1), blocks[1].recovered_block().num_hash());
        assert_eq!(in_memory_state.removal_generation(), generation + 1);
        assert_eq!(state.canonical_chain().count(), 0);
        for removed in [hash(1), hash(2), fork_hash] {
            assert_eq!(in_memory_state.state_by_hash(removed), None);
        }
        assert_eq!(in_memory_state.state_by_hash(hash(3)).unwrap().hash(), hash(3));
    }

    #[test]
    fn test_on_backfill_finished_at_conflicting_block() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let fork = block_builder.get_executed_block_with_number(2, hash(1));
        let fork_hash = fork.recovered_block().hash();
        let fork_child = block_builder.get_executed_block_with_number(3, fork_hash);
        let fork_child_hash = fork_child.recovered_block().hash();
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        state.insert_executed(fork.clone());
        state.insert_executed(fork_child);

        // Backfill finishes at the fork block of height 2: canonical block 2 conflicts with it
        // and is removed together with block 3 built on it, and the fork stays tracked.
        let generation = in_memory_state.removal_generation();
        state.on_backfill_finished(hash(1), fork.recovered_block().num_hash());
        assert_eq!(in_memory_state.removal_generation(), generation + 1);
        assert_eq!(state.canonical_chain().count(), 0);
        for removed in [hash(1), hash(2), hash(3)] {
            assert_eq!(in_memory_state.state_by_hash(removed), None);
        }
        for kept in [fork_hash, fork_child_hash] {
            assert_eq!(in_memory_state.state_by_hash(kept).unwrap().hash(), kept);
        }
    }

    #[test]
    fn test_on_blocks_persisted() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let num_hash = |number: usize| blocks[number - 1].recovered_block().num_hash();
        // One fork at the height of each of the blocks 2, 3 and 4.
        let forks = (2..5)
            .map(|number| {
                block_builder.get_executed_block_with_number(number as u64, hash(number - 1))
            })
            .collect::<Vec<_>>();
        let fork_hash = |number: usize| forks[number - 2].recovered_block().hash();
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        for fork in &forks {
            state.insert_executed(fork.clone());
        }

        // Without a finalized block only the canonical blocks up to the state trie frontier are
        // removed, in one update.
        let generation = in_memory_state.removal_generation();
        state.on_blocks_persisted(num_hash(2), num_hash(1), None);
        assert_eq!(in_memory_state.removal_generation(), generation + 1);
        assert_eq!(state.get_persisted_num_hash(), Some(num_hash(2)));
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 4, non_canonical: 3 }
        );
        assert!(!in_memory_state.contains(&hash(1)));

        // A finalized block above the state trie frontier is replaced by the frontier: the fork
        // at the height of block 2 conflicts with it, the ones above do not.
        state.on_blocks_persisted(num_hash(2), num_hash(2), Some(num_hash(4)));
        assert_eq!(in_memory_state.removal_generation(), generation + 2);
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 2 }
        );
        assert!(!in_memory_state.contains(&fork_hash(2)));

        // A finalized block below the frontier is used as it is.
        state.on_blocks_persisted(num_hash(4), num_hash(4), Some(num_hash(3)));
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 1 }
        );
        assert!(!in_memory_state.contains(&fork_hash(3)));
        assert!(in_memory_state.contains(&fork_hash(4)));
    }

    #[test]
    fn test_on_blocks_persisted_after_disk_removal() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..9).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let num_hash = |number: usize| blocks[number - 1].recovered_block().num_hash();
        let fork = block_builder.get_executed_block_with_number(7, hash(6));
        let fork_hash = fork.recovered_block().hash();
        let fork_child = block_builder.get_executed_block_with_number(8, fork_hash);

        // Block 8 is loaded from disk as the canonical head, and replaced by block 6 before its
        // parent is loaded: it is tracked, but block 7 is only on disk.
        state.update_chain(NewCanonicalChain::Commit { new: vec![blocks[7].clone()] });
        state.update_chain(NewCanonicalChain::Reorg {
            new: vec![blocks[5].clone()],
            old: vec![blocks[7].clone()],
        });
        state.insert_executed(fork);
        state.insert_executed(fork_child.clone());
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 3 }
        );

        // The blocks above block 6 are removed from disk. Block 8 builds on block 7, which is
        // gone, so it is removed. The fork builds on block 6 and stays.
        state.on_blocks_persisted(num_hash(6), num_hash(6), None);
        assert!(!in_memory_state.contains(&hash(8)));
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 2 }
        );
        assert_eq!(
            in_memory_state.state_by_hash(fork_child.recovered_block().hash()).unwrap().anchor(),
            num_hash(6)
        );
    }

    #[test]
    fn test_in_memory_state_contains_and_block_counts() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let in_memory_state = &state.inner.in_memory_state;
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..3).collect::<Vec<_>>();
        let fork =
            block_builder.get_executed_block_with_number(2, blocks[0].recovered_block().hash());
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        state.insert_executed(fork.clone());

        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 2, non_canonical: 1 }
        );
        for block in blocks.iter().chain([&fork]) {
            assert!(in_memory_state.contains(&block.recovered_block().hash()));
        }
        assert!(!in_memory_state.contains(&B256::random()));
    }

    #[test]
    fn test_writer_changes_blocks_of_canonical_state() {
        let mut writer = CanonicalInMemoryStateWriter::new(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            InMemoryState::default(),
        );
        let state: CanonicalInMemoryState = writer.canonical_in_memory_state().clone();
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let hash = |number: usize| blocks[number - 1].recovered_block().hash();
        let num_hash = |number: usize| blocks[number - 1].recovered_block().num_hash();
        let fork = block_builder.get_executed_block_with_number(2, hash(1));
        let fork_hash = fork.recovered_block().hash();
        let in_memory_state = &state.inner.in_memory_state;

        // Blocks written through the writer are read through the state it was created for.
        writer.insert_executed(blocks[0].clone());
        writer.set_pending_block(blocks[1].clone());
        assert_eq!(state.pending_block_num_hash(), Some(num_hash(2)));
        writer.update_chain(NewCanonicalChain::Commit { new: blocks[..2].to_vec() });
        writer.insert_executed(blocks[2].clone());
        writer.insert_executed(fork);
        assert_eq!(state.canonical_chain().count(), 2);
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 2, non_canonical: 2 }
        );

        // Persisting the blocks 1 and 2 removes them, and finalizing block 2 removes the fork at
        // its height.
        writer.on_blocks_persisted(num_hash(2), num_hash(2), Some(num_hash(2)));
        assert_eq!(state.get_persisted_num_hash(), Some(num_hash(2)));
        for removed in [hash(1), hash(2), fork_hash] {
            assert!(!in_memory_state.contains(&removed));
        }

        // After a backfill run that ends at block 2, block 3 is no longer canonical.
        writer.update_chain(NewCanonicalChain::Commit { new: vec![blocks[2].clone()] });
        assert_eq!(state.canonical_chain().count(), 1);
        writer.on_backfill_finished(hash(2), num_hash(2));
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 1 }
        );

        writer.clear_state();
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 0 }
        );

        // The chain info set through the writer is read through the state as well.
        let header = blocks[2].recovered_block().clone_sealed_header();
        writer.on_forkchoice_update_received();
        writer.set_canonical_head(header.clone());
        writer.set_safe(header.clone());
        writer.set_finalized(header.clone());
        assert!(state.last_received_update_timestamp().is_some());
        assert_eq!(state.get_canonical_head(), header);
        assert_eq!(state.get_safe_num_hash(), Some(header.num_hash()));
        assert_eq!(state.get_finalized_num_hash(), Some(header.num_hash()));
    }
}
