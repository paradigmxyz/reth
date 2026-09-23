//! Types for tracking the canonical chain state in memory.

use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications, ChainInfoTracker,
};
use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_eips::{BlockHashOrNumber, BlockNumHash};
use alloy_primitives::{
    map::{B256Map, B256Set},
    BlockNumber, TxHash, B256,
};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::{
    BlockExecutionOutput, BlockExecutionResult, Chain, DecodedRevmBal, ExecutionOutcome,
};
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives_traits::{
    BlockBody as _, IndexedTx, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
    SignedTransaction,
};
use reth_trie::{
    updates::TrieUpdatesSorted, ComputedTrieData, HashedPostStateSorted, LazyTrieData,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
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
    /// The number of canonical blocks in the in-memory state.
    pub(crate) num_blocks: Gauge,
    /// The number of executed in-memory blocks that are not canonical.
    pub(crate) num_pending_blocks: Gauge,
}

/// Block counts of the in-memory state, taken under the lock and recorded after releasing it.
#[derive(Debug, Clone, Copy)]
struct InMemoryStateStats {
    earliest: Option<BlockNumber>,
    latest: Option<BlockNumber>,
    canonical: usize,
    pending: usize,
}

/// The canonical chain above the persisted frontier.
#[derive(Debug, Default)]
struct CanonicalBlocks<N: NodePrimitives> {
    /// Canonical blocks by hash.
    blocks: B256Map<Arc<BlockState<N>>>,
    /// Canonical block hashes by number.
    numbers: BTreeMap<BlockNumber, B256>,
}

impl<N: NodePrimitives> CanonicalBlocks<N> {
    fn head(&self) -> Option<&Arc<BlockState<N>>> {
        self.numbers.last_key_value().and_then(|(_, hash)| self.blocks.get(hash))
    }

    /// Returns the hash of the block the in-memory canonical chain builds on.
    fn anchor_hash(&self) -> Option<B256> {
        self.numbers
            .first_key_value()
            .and_then(|(_, hash)| self.blocks.get(hash))
            .map(|state| state.parent_hash())
    }

    /// Inserts a canonical block and returns the canonical block it displaces at its height.
    fn insert(&mut self, state: Arc<BlockState<N>>) -> Option<Arc<BlockState<N>>> {
        let hash = state.hash();
        let displaced = self
            .numbers
            .insert(state.number(), hash)
            .filter(|displaced| *displaced != hash)
            .and_then(|displaced| self.blocks.remove(&displaced));
        self.blocks.insert(hash, state);
        displaced
    }

    fn remove(&mut self, hash: &B256) -> Option<Arc<BlockState<N>>> {
        let state = self.blocks.remove(hash)?;
        if self.numbers.get(&state.number()) == Some(hash) {
            self.numbers.remove(&state.number());
        }
        Some(state)
    }

    /// Returns the canonical block built on `parent`, if there is one.
    fn child_of(&self, parent: &BlockState<N>) -> Option<B256> {
        self.numbers.get(&(parent.number() + 1)).copied().filter(|hash| {
            self.blocks.get(hash).is_some_and(|child| child.parent_hash() == parent.hash())
        })
    }
}

/// Executed blocks that are not canonical.
///
/// These are blocks that were validated but are not (yet) part of the canonical chain: payloads
/// that await a forkchoice update, blocks of forks, and blocks a reorg moved off the canonical
/// chain. They stay here until they become canonical or can no longer become canonical.
#[derive(Debug, Default)]
struct PendingBlocks<N: NodePrimitives> {
    /// Pending blocks by hash.
    blocks: B256Map<Arc<BlockState<N>>>,
    /// Pending block hashes by number. Forks can put several blocks at the same height.
    numbers: BTreeMap<BlockNumber, B256Set>,
    /// Pending block hashes by parent hash. The parent can be pending, canonical or persisted.
    children: B256Map<B256Set>,
}

impl<N: NodePrimitives> PendingBlocks<N> {
    fn insert(&mut self, state: Arc<BlockState<N>>) {
        let hash = state.hash();
        self.numbers.entry(state.number()).or_default().insert(hash);
        self.children.entry(state.parent_hash()).or_default().insert(hash);
        self.blocks.insert(hash, state);
    }

    fn remove(&mut self, hash: &B256) -> Option<Arc<BlockState<N>>> {
        let state = self.blocks.remove(hash)?;
        if let Some(hashes) = self.numbers.get_mut(&state.number()) {
            hashes.remove(hash);
            if hashes.is_empty() {
                self.numbers.remove(&state.number());
            }
        }
        if let Some(hashes) = self.children.get_mut(&state.parent_hash()) {
            hashes.remove(hash);
            if hashes.is_empty() {
                self.children.remove(&state.parent_hash());
            }
        }
        Some(state)
    }

    fn children_of(&self, parent_hash: B256) -> impl Iterator<Item = B256> + '_ {
        self.children.get(&parent_hash).into_iter().flatten().copied()
    }
}

/// All executed in-memory blocks, split into the canonical and the pending section.
///
/// Every block lives in exactly one section. Its parent link points at the [`BlockState`] this
/// type holds for the parent, in either section, or is `None` if the parent is not in memory.
#[derive(Debug, Default)]
struct Blocks<N: NodePrimitives> {
    /// The canonical chain above the persisted frontier.
    canonical: CanonicalBlocks<N>,
    /// Executed blocks that are not canonical.
    pending: PendingBlocks<N>,
    /// Hash of the block served as the pending block, see
    /// [`CanonicalInMemoryState::set_pending_block`].
    pending_block: Option<B256>,
}

impl<N: NodePrimitives> Blocks<N> {
    fn get(&self, hash: &B256) -> Option<&Arc<BlockState<N>>> {
        self.canonical.blocks.get(hash).or_else(|| self.pending.blocks.get(hash))
    }

    fn contains(&self, hash: &B256) -> bool {
        self.canonical.blocks.contains_key(hash) || self.pending.blocks.contains_key(hash)
    }

    fn pending_state(&self) -> Option<Arc<BlockState<N>>> {
        self.pending_block.and_then(|hash| self.get(&hash)).cloned()
    }

    fn stats(&self) -> InMemoryStateStats {
        InMemoryStateStats {
            earliest: self.canonical.numbers.first_key_value().map(|(number, _)| *number),
            latest: self.canonical.numbers.last_key_value().map(|(number, _)| *number),
            canonical: self.canonical.blocks.len(),
            pending: self.pending.blocks.len(),
        }
    }

    /// Returns the hashes of all blocks, in either section, that are built on `parent`.
    fn children_of<'a>(&'a self, parent: &BlockState<N>) -> impl Iterator<Item = B256> + 'a {
        self.canonical.child_of(parent).into_iter().chain(self.pending.children_of(parent.hash()))
    }

    /// Replaces the state of a tracked block, keeping it in its section.
    fn replace(&mut self, state: Arc<BlockState<N>>) {
        let hash = state.hash();
        if let Some(existing) = self.canonical.blocks.get_mut(&hash) {
            *existing = state;
        } else if let Some(existing) = self.pending.blocks.get_mut(&hash) {
            *existing = state;
        }
    }

    /// Re-links every descendant of `parents` whose parent link no longer points at the state
    /// this type holds for its parent.
    ///
    /// This is required after a block was removed, which must not stay reachable through the
    /// parent links of the blocks built on it, and after a block was inserted that other tracked
    /// blocks are already built on. A re-linked block gets a new [`BlockState`], so its
    /// descendants are re-linked as well.
    fn relink_children<'a>(&mut self, parents: impl IntoIterator<Item = &'a Arc<BlockState<N>>>)
    where
        N: 'a,
    {
        let mut queue = parents
            .into_iter()
            .flat_map(|parent| self.children_of(parent))
            .collect::<VecDeque<_>>();
        while let Some(hash) = queue.pop_front() {
            let Some(state) = self.get(&hash) else { continue };
            let parent = self.get(&state.parent_hash());
            let linked = match (state.parent.as_ref(), parent) {
                (None, None) => true,
                (Some(linked), Some(parent)) => Arc::ptr_eq(linked, parent),
                _ => false,
            };
            if linked {
                continue
            }
            let relinked = Arc::new(BlockState::with_parent(state.block.clone(), parent.cloned()));
            queue.extend(self.children_of(&relinked));
            self.replace(relinked);
        }
    }

    /// Inserts a block into the pending section unless it is already tracked, and returns its
    /// state.
    fn insert_pending(&mut self, block: ExecutedBlock<N>) -> Arc<BlockState<N>> {
        let hash = block.recovered_block().hash();
        if let Some(existing) = self.get(&hash) {
            return Arc::clone(existing)
        }
        let parent = self.get(&block.recovered_block().parent_hash()).cloned();
        let state = Arc::new(BlockState::with_parent(block, parent));
        self.pending.insert(Arc::clone(&state));
        // Blocks built on this one can already be tracked if it was persisted and trimmed before,
        // for example when a reorg brings it back into memory.
        self.relink_children([&state]);
        state
    }

    /// Makes `new` canonical and moves the canonical blocks of `reorged` to the pending section.
    ///
    /// Blocks that are already tracked keep their state, so everyone holding it keeps sharing
    /// it. Reorged blocks that are not canonical in memory are ignored.
    fn update_chain(&mut self, new: Vec<ExecutedBlock<N>>, reorged: Vec<ExecutedBlock<N>>) {
        for block in reorged {
            if let Some(state) = self.canonical.remove(&block.recovered_block().hash()) {
                self.pending.insert(state);
            }
        }

        let mut inserted = Vec::new();
        for block in new {
            let hash = block.recovered_block().hash();
            let state = if let Some(state) = self.pending.remove(&hash) {
                state
            } else if let Some(state) = self.canonical.blocks.get(&hash) {
                Arc::clone(state)
            } else {
                let parent = self.get(&block.recovered_block().parent_hash()).cloned();
                let state = Arc::new(BlockState::with_parent(block, parent));
                inserted.push(Arc::clone(&state));
                state
            };
            if let Some(displaced) = self.canonical.insert(state) {
                self.pending.insert(displaced);
            }
        }

        self.pending_block = None;
        self.relink_children(&inserted);
    }

    /// Removes canonical blocks up to and including `remove_until`, if `persisted_hash` is part of
    /// the in-memory canonical chain or the block it builds on, and returns their states.
    fn remove_canonical_until(
        &mut self,
        persisted_hash: B256,
        remove_until: BlockNumber,
    ) -> Vec<Arc<BlockState<N>>> {
        // If the persisted hash is not on the canonical chain, canonical blocks were not actually
        // persisted. This can happen if the persistence task takes a long time, while a reorg is
        // happening.
        if !self.canonical.blocks.contains_key(&persisted_hash) &&
            self.canonical.anchor_hash() != Some(persisted_hash)
        {
            return Vec::new()
        }

        let hashes = self
            .canonical
            .numbers
            .range(..=remove_until)
            .map(|(_, hash)| *hash)
            .collect::<Vec<_>>();
        let removed =
            hashes.iter().filter_map(|hash| self.canonical.remove(hash)).collect::<Vec<_>>();
        self.clear_untracked_pending_block();
        self.relink_children(&removed);
        removed
    }

    /// Removes pending blocks that can never become canonical once `finalized` is final, and
    /// returns their states.
    ///
    /// These are all pending blocks below the finalized block, all other pending blocks at its
    /// height and all blocks built on those.
    fn prune_pending_below(&mut self, finalized: BlockNumHash) -> Vec<Arc<BlockState<N>>> {
        let BlockNumHash { number: finalized_number, hash: finalized_hash } = finalized;

        let below = self
            .pending
            .numbers
            .range(..finalized_number)
            .flat_map(|(_, hashes)| hashes.iter().copied())
            .collect::<Vec<_>>();
        let mut removed =
            below.iter().filter_map(|hash| self.pending.remove(hash)).collect::<Vec<_>>();

        let mut forks = self
            .pending
            .numbers
            .get(&finalized_number)
            .into_iter()
            .flatten()
            .copied()
            .filter(|hash| *hash != finalized_hash)
            .collect::<VecDeque<_>>();
        while let Some(hash) = forks.pop_front() {
            if let Some(state) = self.pending.remove(&hash) {
                forks.extend(self.pending.children_of(hash));
                removed.push(state);
            }
        }

        self.clear_untracked_pending_block();
        self.relink_children(&removed);
        removed
    }

    /// Clears the pending block if it is no longer tracked.
    fn clear_untracked_pending_block(&mut self) {
        if self.pending_block.is_some_and(|hash| !self.contains(&hash)) {
            self.pending_block = None;
        }
    }

    /// Moves every canonical block to the pending section and clears the pending block.
    fn demote_canonical(&mut self) {
        let canonical = std::mem::take(&mut self.canonical);
        for state in canonical.blocks.into_values() {
            self.pending.insert(state);
        }
        self.pending_block = None;
    }

    /// Returns the hashes of all blocks, canonical first.
    fn into_hashes(self) -> Vec<B256> {
        self.canonical.blocks.into_keys().chain(self.pending.blocks.into_keys()).collect()
    }
}

/// Container type for the executed blocks that are not on disk yet.
///
/// This tracks the canonical chain that can be traced back to a canonical block on disk, and all
/// other executed blocks that can still become canonical, see [`CanonicalInMemoryState`].
///
/// # Locking behavior on state updates
///
/// Both sections and the pending block are guarded by a single lock. Updates take the write lock
/// once, so readers never observe a block in neither or both sections, or a partially applied
/// update. Metrics are recorded, and removed blocks dropped, after the lock is released.
#[derive(Debug, Default)]
pub(crate) struct InMemoryState<N: NodePrimitives = EthPrimitives> {
    /// The canonical and the pending section.
    blocks: RwLock<Blocks<N>>,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

impl<N: NodePrimitives> InMemoryState<N> {
    pub(crate) fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
    ) -> Self {
        let mut state = Blocks {
            canonical: CanonicalBlocks { blocks, numbers },
            pending: PendingBlocks::default(),
            pending_block: None,
        };
        if let Some(pending) = pending {
            state.pending_block = Some(pending.hash());
            state.pending.insert(Arc::new(pending));
        }
        let stats = state.stats();
        let this = Self { blocks: RwLock::new(state), metrics: Default::default() };
        this.record_metrics(stats);
        this
    }

    /// Applies `f` under the write lock and records the metrics after releasing it.
    ///
    /// Removals return the removed states, so that they are dropped after the lock is released:
    /// the store can hold the last reference to a block and its execution output.
    fn update<R>(&self, f: impl FnOnce(&mut Blocks<N>) -> R) -> R {
        let (result, stats) = {
            let mut blocks = self.blocks.write();
            let result = f(&mut blocks);
            (result, blocks.stats())
        };
        self.record_metrics(stats);
        result
    }

    /// Records the metrics for the in-memory state.
    fn record_metrics(&self, stats: InMemoryStateStats) {
        if let Some(earliest_block_number) = stats.earliest {
            self.metrics.earliest_block.set(earliest_block_number as f64);
        }
        if let Some(latest_block_number) = stats.latest {
            self.metrics.latest_block.set(latest_block_number as f64);
        }
        self.metrics.num_blocks.set(stats.canonical as f64);
        self.metrics.num_pending_blocks.set(stats.pending as f64);
    }

    /// Returns the state for a given canonical block hash.
    pub(crate) fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().canonical.blocks.get(&hash).cloned()
    }

    /// Returns the state for a given block hash, canonical or pending.
    pub(crate) fn executed_state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().get(&hash).cloned()
    }

    /// Returns the state for a given canonical block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        let blocks = self.blocks.read();
        blocks
            .canonical
            .numbers
            .get(&number)
            .and_then(|hash| blocks.canonical.blocks.get(hash))
            .cloned()
    }

    /// Returns the hash for a specific canonical block number
    pub(crate) fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.blocks.read().canonical.numbers.get(&number).copied()
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().canonical.head().cloned()
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().pending_state()
    }
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner<N: NodePrimitives> {
    /// Tracks certain chain information, such as the canonical head, safe head, and finalized
    /// head.
    pub(crate) chain_info_tracker: ChainInfoTracker<N>,
    /// Tracks the executed blocks that have not been persisted to disk yet.
    pub(crate) in_memory_state: InMemoryState<N>,
    /// A broadcast stream that emits events when the canonical chain is updated.
    pub(crate) canon_state_notification_sender: CanonStateNotificationSender<N>,
}

type PendingBlockAndReceipts<N> =
    (RecoveredBlock<<N as NodePrimitives>::Block>, Vec<reth_primitives_traits::ReceiptTy<N>>);

/// This type is responsible for providing the blocks, receipts, and state for all executed blocks
/// that are not on disk yet.
///
/// It is the only place that tracks executed in-memory blocks, in two sections:
///
/// - **canonical**: the canonical chain above the persisted frontier, which can be traced back to a
///   canonical block on disk.
/// - **pending**: every other executed block that can still become canonical: payloads that await a
///   forkchoice update, fork blocks, and blocks a reorg moved off the canonical chain.
///
/// Every block is tracked as one shared [`BlockState`] whose parent link points at its parent's
/// state in either section. Making a block canonical moves that state between the sections
/// instead of rebuilding it. The pending block served over RPC, see [`Self::set_pending_block`],
/// is one of the blocks in the pending section.
///
/// Clones share the same state. A node keeps a single instance that the engine, the providers
/// and the state overlay manager all hold.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState<N: NodePrimitives = EthPrimitives> {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner<N>>,
}

impl<N: NodePrimitives> Default for CanonicalInMemoryState<N> {
    fn default() -> Self {
        Self::empty()
    }
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
        let chain_info_tracker = ChainInfoTracker::new(header, finalized, safe);
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

    /// Create a new in memory state with the given local head and finalized header
    /// if it exists.
    pub fn with_head(
        head: SealedHeader<N::BlockHeader>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let chain_info_tracker = ChainInfoTracker::new(head, finalized, safe);
        let in_memory_state = InMemoryState::default();
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);
        let inner = CanonicalInMemoryStateInner {
            chain_info_tracker,
            in_memory_state,
            canon_state_notification_sender,
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns `true` if both handles share the same state.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Sets the canonical head and, if they are known, the finalized and safe headers.
    pub fn set_head_markers(
        &self,
        head: SealedHeader<N::BlockHeader>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) {
        self.set_canonical_head(head);
        if let Some(finalized) = finalized {
            self.set_finalized(finalized);
        }
        if let Some(safe) = safe {
            self.set_safe(safe);
        }
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

    /// Removes all blocks, canonical and pending, and returns their hashes.
    pub fn clear_state(&self) -> Vec<B256> {
        self.inner.in_memory_state.update(std::mem::take).into_hashes()
    }

    /// Moves every canonical block to the pending section and clears the pending block.
    ///
    /// This is used when the canonical head is reset to a persisted block, e.g. after a backfill
    /// run: none of the in-memory blocks is canonical anymore, but they remain fork candidates.
    pub fn demote_canonical_chain(&self) {
        self.inner.in_memory_state.update(Blocks::demote_canonical)
    }

    /// Inserts an executed block into the pending section and returns its state.
    ///
    /// The block is linked to its parent's state if the parent is in memory, canonical or not.
    /// If the block is already tracked, its existing state is returned.
    pub fn insert_pending(&self, block: ExecutedBlock<N>) -> Arc<BlockState<N>> {
        self.inner.in_memory_state.update(|blocks| blocks.insert_pending(block))
    }

    /// Updates the pending block with the given block.
    ///
    /// The pending block is one of the blocks in the pending section, typically the child of the
    /// canonical head the engine validated last. The block is inserted into the pending section
    /// unless it is already tracked.
    pub fn set_pending_block(&self, pending: ExecutedBlock<N>) {
        self.inner.in_memory_state.update(|blocks| {
            let hash = pending.recovered_block().hash();
            blocks.insert_pending(pending);
            blocks.pending_block = Some(hash);
        })
    }

    /// Update the in memory state with the given chain update.
    ///
    /// This moves the new blocks from the pending to the canonical section and the reorged blocks
    /// from the canonical to the pending section, and clears the pending block. New blocks that
    /// are not tracked yet are inserted and linked to their parents.
    pub fn update_chain(&self, new_chain: NewCanonicalChain<N>) {
        let (new, reorged) = match new_chain {
            NewCanonicalChain::Commit { new } => (new, Vec::new()),
            NewCanonicalChain::Reorg { new, old } => (new, old),
        };
        self.inner.in_memory_state.update(|blocks| blocks.update_chain(new, reorged))
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
    pub fn remove_persisted_blocks_until(
        &self,
        persisted_num_hash: BlockNumHash,
        remove_until: BlockNumber,
    ) {
        self.set_persisted(persisted_num_hash);
        self.remove_canonical_blocks_until(
            persisted_num_hash.hash,
            remove_until.min(persisted_num_hash.number),
        );
    }

    /// Removes canonical blocks up to and including `remove_until` and returns their hashes.
    ///
    /// This only removes blocks if `persisted_hash` is part of the in-memory canonical chain, or
    /// is the block it builds on. Otherwise canonical blocks were not actually persisted, which
    /// can happen if the persistence task takes a long time while a reorg is happening.
    ///
    /// The remaining blocks of both sections are re-linked, so that no removed block stays
    /// reachable through their parent links.
    pub fn remove_canonical_blocks_until(
        &self,
        persisted_hash: B256,
        remove_until: BlockNumber,
    ) -> Vec<B256> {
        let removed = self
            .inner
            .in_memory_state
            .update(|blocks| blocks.remove_canonical_until(persisted_hash, remove_until));
        removed.iter().map(|state| state.hash()).collect()
    }

    /// Removes pending blocks that can never become canonical with `finalized` finalized, and
    /// returns their hashes.
    ///
    /// These are all pending blocks below the finalized block, all other pending blocks at its
    /// height, and all blocks built on those.
    pub fn prune_pending_below(&self, finalized: BlockNumHash) -> Vec<B256> {
        let removed =
            self.inner.in_memory_state.update(|blocks| blocks.prune_pending_below(finalized));
        removed.iter().map(|state| state.hash()).collect()
    }

    /// Returns in memory state corresponding the given hash.
    ///
    /// This only returns canonical blocks, see [`Self::executed_state_by_hash`] for all blocks.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    /// Returns the state of the executed in-memory block with the given hash, canonical or
    /// pending.
    pub fn executed_state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.executed_state_by_hash(hash)
    }

    /// Returns the states of all pending blocks built on `parent_hash`.
    pub fn pending_children(&self, parent_hash: B256) -> Vec<Arc<BlockState<N>>> {
        let blocks = self.inner.in_memory_state.blocks.read();
        blocks
            .pending
            .children_of(parent_hash)
            .filter_map(|hash| blocks.pending.blocks.get(&hash))
            .cloned()
            .collect()
    }

    /// Returns the states of all executed in-memory blocks at the given height, canonical first.
    pub fn blocks_at_number(&self, number: BlockNumber) -> Vec<Arc<BlockState<N>>> {
        let blocks = self.inner.in_memory_state.blocks.read();
        let canonical = blocks.canonical.numbers.get(&number).into_iter();
        let pending = blocks.pending.numbers.get(&number).into_iter().flatten();
        canonical.chain(pending).filter_map(|hash| blocks.get(hash)).cloned().collect()
    }

    /// Returns the number of blocks in the canonical section.
    pub fn canonical_block_count(&self) -> usize {
        self.inner.in_memory_state.blocks.read().canonical.blocks.len()
    }

    /// Returns the number of blocks in the pending section.
    pub fn pending_block_count(&self) -> usize {
        self.inner.in_memory_state.blocks.read().pending.blocks.len()
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
    pub fn pending_recovered_block(&self) -> Option<RecoveredBlock<N::Block>>
    where
        N::SignedTx: SignedTransaction,
    {
        self.pending_state().map(|block_state| block_state.block_ref().recovered_block().clone())
    }

    /// Returns a tuple with the `SealedBlock` corresponding to the pending
    /// state and a vector of its `Receipt`s.
    pub fn pending_block_and_receipts(&self) -> Option<PendingBlockAndReceipts<N>> {
        self.pending_state().map(|block_state| {
            (
                block_state.block_ref().recovered_block().clone(),
                block_state.executed_block_receipts(),
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

/// State after applying the given block, which is part of a chain that is partially stored in
/// memory and can be traced back to a block on disk.
///
/// The parent link points at the state of the parent block while the parent is in memory.
#[derive(Debug, Clone)]
pub struct BlockState<N: NodePrimitives = EthPrimitives> {
    /// The executed block that determines the state after this block has been executed.
    block: ExecutedBlock<N>,
    /// The block's parent block if it exists.
    parent: Option<Arc<Self>>,
}

impl<N: NodePrimitives> PartialEq for BlockState<N> {
    fn eq(&self, other: &Self) -> bool {
        self.block == other.block && self.parent == other.parent
    }
}

impl<N: NodePrimitives> BlockState<N> {
    /// [`BlockState`] constructor.
    pub const fn new(block: ExecutedBlock<N>) -> Self {
        Self { block, parent: None }
    }

    /// [`BlockState`] constructor with parent.
    pub const fn with_parent(block: ExecutedBlock<N>, parent: Option<Arc<Self>>) -> Self {
        Self { block, parent }
    }

    /// Returns the hash and block of the on disk block this state can be traced back to.
    pub fn anchor(&self) -> BlockNumHash {
        let mut current = self;
        while let Some(parent) = &current.parent {
            current = parent;
        }
        current.block.recovered_block().parent_num_hash()
    }

    /// Returns the hash of the parent block.
    fn parent_hash(&self) -> B256 {
        self.block.recovered_block().parent_hash()
    }

    /// Returns the executed block that determines the state.
    pub fn block(&self) -> ExecutedBlock<N> {
        self.block.clone()
    }

    /// Returns a reference to the executed block that determines the state.
    pub const fn block_ref(&self) -> &ExecutedBlock<N> {
        &self.block
    }

    /// Returns the hash of executed block that determines the state.
    pub fn hash(&self) -> B256 {
        self.block.recovered_block().hash()
    }

    /// Returns the block number of executed block that determines the state.
    pub fn number(&self) -> u64 {
        self.block.recovered_block().number()
    }

    /// Returns the state root after applying the executed block that determines
    /// the state.
    pub fn state_root(&self) -> B256 {
        self.block.recovered_block().state_root()
    }

    /// Returns the `Receipts` of executed block that determines the state.
    pub fn receipts(&self) -> &Vec<N::Receipt> {
        &self.block.execution_outcome().receipts
    }

    /// Returns a vector of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    ///
    /// This clones the vector of receipts. To avoid it, use [`Self::executed_block_receipts_ref`].
    pub fn executed_block_receipts(&self) -> Vec<N::Receipt> {
        self.receipts().clone()
    }

    /// Returns a slice of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    pub fn executed_block_receipts_ref(&self) -> &[N::Receipt] {
        self.receipts()
    }

    /// Returns an iterator over __parent__ `BlockStates`.
    ///
    /// The block state order is newest to oldest (highest to lowest):
    /// `[5,4,3,2,1]`
    ///
    /// Note: This does not include self.
    pub fn parent_state_chain(&self) -> impl Iterator<Item = &Self> + '_ {
        std::iter::successors(self.parent.as_deref(), |state| state.parent.as_deref())
    }

    /// Returns a vector of `BlockStates` representing the entire in memory chain.
    /// The block state order in the output vector is newest to oldest (highest to lowest),
    /// including self as the first element.
    pub fn chain(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |state| state.parent.as_deref())
    }

    /// Appends the parent chain of this [`BlockState`] to the given vector.
    ///
    /// Parents are appended in order from newest to oldest (highest to lowest).
    /// This does not include self, only the parent states.
    ///
    /// This is a convenience method equivalent to `chain.extend(self.parent_state_chain())`.
    pub fn append_parent_chain<'a>(&'a self, chain: &mut Vec<&'a Self>) {
        chain.extend(self.parent_state_chain());
    }

    /// Returns an iterator over the atomically captured chain of in memory blocks.
    ///
    /// This yields the blocks from newest to oldest (highest to lowest).
    pub fn iter(self: Arc<Self>) -> impl Iterator<Item = Arc<Self>> {
        std::iter::successors(Some(self), |state| state.parent.clone())
    }

    /// Tries to find a block by [`BlockHashOrNumber`] in the chain ending at this block.
    pub fn block_on_chain(&self, hash_or_num: BlockHashOrNumber) -> Option<&Self> {
        self.chain().find(|block| match hash_or_num {
            BlockHashOrNumber::Hash(hash) => block.hash() == hash,
            BlockHashOrNumber::Number(number) => block.number() == number,
        })
    }

    /// Tries to find a transaction by [`TxHash`] in the chain ending at this block.
    pub fn transaction_on_chain(&self, hash: TxHash) -> Option<N::SignedTx> {
        self.chain().find_map(|block_state| {
            block_state.block_ref().recovered_block().body().transaction_by_hash(&hash).cloned()
        })
    }

    /// Tries to find a transaction with meta by [`TxHash`] in the chain ending at this block.
    pub fn transaction_meta_on_chain(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        self.chain().find_map(|block_state| {
            block_state.find_indexed(tx_hash).map(|indexed| (indexed.tx().clone(), indexed.meta()))
        })
    }

    /// Finds a transaction by hash and returns it with its index and block context.
    pub fn find_indexed(&self, tx_hash: TxHash) -> Option<IndexedTx<'_, N::Block>> {
        self.block_ref().recovered_block().find_indexed(tx_hash)
    }
}

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug)]
pub struct ExecutedBlock<N: NodePrimitives = EthPrimitives> {
    /// Recovered Block
    pub recovered_block: Arc<RecoveredBlock<N::Block>>,
    /// Block's execution outcome.
    pub execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
    /// Deferred trie data produced by execution.
    ///
    /// This allows deferring the computation of the trie data which can be expensive.
    /// The data can be populated asynchronously after the block was validated.
    pub trie_data: LazyTrieData,
    /// The prepared block access list of the block, if one is available.
    ///
    /// `None` means no BAL was available when the block was constructed, not that the block
    /// has none: only blocks the engine validated from a payload that carried a BAL have it
    /// attached. Blocks from before the BAL fork, blocks built or loaded outside engine
    /// validation (payload builder, persistence, tests) and downloaded blocks without a BAL
    /// sidecar leave it unset; the BAL store is the source of truth in that case.
    ///
    /// When present, this carries the raw RLP (for the BAL store) together with the revm
    /// representation (for consumers like the RPC state cache), so that neither has to be
    /// re-derived after validation.
    pub bal: Option<Arc<DecodedRevmBal>>,
}

impl<N: NodePrimitives> Default for ExecutedBlock<N> {
    fn default() -> Self {
        Self {
            recovered_block: Default::default(),
            execution_output: Arc::new(BlockExecutionOutput {
                result: BlockExecutionResult {
                    receipts: Default::default(),
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
                state: Default::default(),
            }),
            trie_data: LazyTrieData::ready(ComputedTrieData::default()),
            bal: None,
        }
    }
}

impl<N: NodePrimitives> PartialEq for ExecutedBlock<N> {
    fn eq(&self, other: &Self) -> bool {
        // Trie data is computed asynchronously and the block access list is derived data; neither
        // defines block identity.
        self.recovered_block == other.recovered_block &&
            self.execution_output == other.execution_output
    }
}

impl<N: NodePrimitives> ExecutedBlock<N> {
    /// Create a new [`ExecutedBlock`] with already-computed trie data.
    ///
    /// Use this constructor when trie data is available immediately (e.g., sequencers,
    /// payload builders). This is the safe default path.
    pub fn new(
        recovered_block: Arc<RecoveredBlock<N::Block>>,
        execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
        trie_data: ComputedTrieData,
    ) -> Self {
        Self {
            recovered_block,
            execution_output,
            trie_data: LazyTrieData::ready(trie_data),
            bal: None,
        }
    }

    /// Create a new [`ExecutedBlock`] with deferred trie data.
    ///
    /// This is useful if the trie data is populated somewhere else, e.g. asynchronously
    /// after the block was validated.
    ///
    /// The [`LazyTrieData`] handle allows expensive trie operations (sorting hashed state and
    /// trie updates) to be performed outside the critical validation path by a background task.
    /// This can improve latency for time-sensitive operations like block validation.
    ///
    /// If the data hasn't been populated when [`Self::trie_data()`] is called, the caller waits
    /// for the background task to publish it.
    ///
    /// Use [`Self::new()`] instead when trie data is already computed and available immediately.
    pub const fn with_deferred_trie_data(
        recovered_block: Arc<RecoveredBlock<N::Block>>,
        execution_output: Arc<BlockExecutionOutput<N::Receipt>>,
        trie_data: LazyTrieData,
    ) -> Self {
        Self { recovered_block, execution_output, trie_data, bal: None }
    }

    /// Attaches the prepared block access list of the block, or clears it with `None`.
    pub fn with_bal(mut self, bal: Option<Arc<DecodedRevmBal>>) -> Self {
        self.bal = bal;
        self
    }

    /// Returns the prepared block access list of the block, if one is available.
    ///
    /// `None` only means no BAL was attached to this block; see the `bal` field for details.
    #[inline]
    pub const fn bal(&self) -> Option<&Arc<DecodedRevmBal>> {
        self.bal.as_ref()
    }

    /// Returns a reference to an inner [`SealedBlock`]
    #[inline]
    pub fn sealed_block(&self) -> &SealedBlock<N::Block> {
        self.recovered_block.sealed_block()
    }

    /// Returns a reference to [`RecoveredBlock`]
    #[inline]
    pub fn recovered_block(&self) -> &RecoveredBlock<N::Block> {
        &self.recovered_block
    }

    /// Returns a reference to the block's execution outcome
    #[inline]
    pub fn execution_outcome(&self) -> &BlockExecutionOutput<N::Receipt> {
        &self.execution_output
    }

    /// Returns the trie data, waiting for the background task if not already cached.
    ///
    /// Uses `OnceLock::get_or_init` internally:
    /// - If already computed: returns cached result immediately
    /// - If not computed: first caller waits for the publishing task, others wait for that result
    #[inline]
    #[tracing::instrument(level = "debug", target = "engine::tree", name = "trie_data", skip_all)]
    pub fn trie_data(&self) -> ComputedTrieData {
        self.trie_data.get().clone()
    }

    /// Returns a clone of the deferred trie data handle.
    ///
    /// A handle is a lightweight reference that can be passed to descendants without
    /// forcing trie data to be observed immediately. The actual work runs in the background task.
    #[inline]
    pub fn trie_data_handle(&self) -> LazyTrieData {
        self.trie_data.clone()
    }

    /// Returns the hashed state result of the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn hashed_state(&self) -> Arc<HashedPostStateSorted> {
        self.trie_data().sorted.hashed_state
    }

    /// Returns a reference to the hashed state result of the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn hashed_state_ref(&self) -> &HashedPostStateSorted {
        &self.trie_data.get().sorted.hashed_state
    }

    /// Returns references to the hashed state results of the executed blocks.
    ///
    /// May wait for trie data if any deferred task hasn't completed.
    pub fn hashed_state_refs(blocks: &[Self]) -> Vec<&HashedPostStateSorted> {
        blocks.iter().map(Self::hashed_state_ref).collect()
    }

    /// Returns the trie updates resulting from the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn trie_updates(&self) -> Arc<TrieUpdatesSorted> {
        self.trie_data().sorted.trie_updates
    }

    /// Returns a reference to the trie updates resulting from the execution outcome.
    ///
    /// May wait for trie data if the deferred task hasn't completed.
    #[inline]
    pub fn trie_updates_ref(&self) -> &TrieUpdatesSorted {
        &self.trie_data.get().sorted.trie_updates
    }

    /// Returns references to the trie updates of the executed blocks.
    ///
    /// May wait for trie data if any deferred task hasn't completed.
    pub fn trie_updates_refs(blocks: &[Self]) -> Vec<&TrieUpdatesSorted> {
        blocks.iter().map(Self::trie_updates_ref).collect()
    }

    /// Returns a [`BlockNumber`] of the block.
    #[inline]
    pub fn block_number(&self) -> BlockNumber {
        self.recovered_block.header().number()
    }
}

/// Non-empty chain of blocks.
#[derive(Debug)]
pub enum NewCanonicalChain<N: NodePrimitives = EthPrimitives> {
    /// A simple append to the current canonical head
    Commit {
        /// all blocks that lead back to the canonical head
        new: Vec<ExecutedBlock<N>>,
    },
    /// A reorged chain consists of two chains that trace back to a shared ancestor block at which
    /// point they diverge.
    Reorg {
        /// All blocks of the _new_ chain
        new: Vec<ExecutedBlock<N>>,
        /// All blocks of the _old_ chain
        old: Vec<ExecutedBlock<N>>,
    },
}

impl<N: NodePrimitives<SignedTx: SignedTransaction>> NewCanonicalChain<N> {
    /// Returns the length of the new chain.
    pub const fn new_block_count(&self) -> usize {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.len(),
        }
    }

    /// Returns the length of the reorged chain.
    pub const fn reorged_block_count(&self) -> usize {
        match self {
            Self::Commit { .. } => 0,
            Self::Reorg { old, .. } => old.len(),
        }
    }

    /// Converts the new chain into a notification that will be emitted to listeners
    pub fn to_chain_notification(&self) -> CanonStateNotification<N> {
        match self {
            Self::Commit { new } => {
                CanonStateNotification::Commit { new: Arc::new(Self::blocks_to_chain(new)) }
            }
            Self::Reorg { new, old } => CanonStateNotification::Reorg {
                new: Arc::new(Self::blocks_to_chain(new)),
                old: Arc::new(Self::blocks_to_chain(old)),
            },
        }
    }

    /// Converts a slice of executed blocks into a [`Chain`].
    fn blocks_to_chain(blocks: &[ExecutedBlock<N>]) -> Chain<N> {
        let mut chain = match blocks {
            [] => Chain::default(),
            [first, rest @ ..] => {
                let mut chain = Chain::from_block(
                    Arc::clone(&first.recovered_block),
                    ExecutionOutcome::from((
                        first.execution_outcome().clone(),
                        first.block_number(),
                    )),
                    first.trie_data_handle(),
                );
                for exec in rest {
                    chain.append_block(
                        Arc::clone(&exec.recovered_block),
                        ExecutionOutcome::from((
                            exec.execution_outcome().clone(),
                            exec.block_number(),
                        )),
                        exec.trie_data_handle(),
                    );
                }
                chain
            }
        };
        for exec in blocks {
            if let Some(bal) = exec.bal() {
                chain.insert_bal(exec.block_number(), Arc::clone(bal));
            }
        }
        chain
    }

    /// Returns the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &RecoveredBlock<N::Block> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => {
                new.last().expect("non empty blocks").recovered_block()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::Bytes;
    use rand::Rng;
    use reth_ethereum_primitives::{EthPrimitives, Receipt};

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Asserts that every block is tracked in exactly one section, that the number and parent
    /// indexes match the tracked blocks, that every parent link points at the state tracked for
    /// the parent, and that the pending block is tracked.
    fn assert_consistent(state: &CanonicalInMemoryState) {
        let blocks = state.inner.in_memory_state.blocks.read();
        let Blocks { canonical, pending, pending_block } = &*blocks;

        assert_eq!(canonical.numbers.len(), canonical.blocks.len());
        for (number, hash) in &canonical.numbers {
            assert_eq!(canonical.blocks[hash].number(), *number);
            assert!(!pending.blocks.contains_key(hash), "block {hash} is in both sections");
        }

        let by_number = pending
            .numbers
            .iter()
            .flat_map(|(number, hashes)| hashes.iter().map(move |hash| (*number, *hash)))
            .collect::<Vec<_>>();
        assert_eq!(by_number.len(), pending.blocks.len());
        for (number, hash) in by_number {
            assert_eq!(pending.blocks[&hash].number(), number);
        }
        let by_parent = pending
            .children
            .iter()
            .flat_map(|(parent, hashes)| hashes.iter().map(move |hash| (*parent, *hash)))
            .collect::<Vec<_>>();
        assert_eq!(by_parent.len(), pending.blocks.len());
        for (parent, hash) in by_parent {
            assert_eq!(pending.blocks[&hash].parent_hash(), parent);
        }
        assert!(pending
            .numbers
            .values()
            .chain(pending.children.values())
            .all(|set| !set.is_empty()));

        for state in canonical.blocks.values().chain(pending.blocks.values()) {
            match (state.parent.as_ref(), blocks.get(&state.parent_hash())) {
                (None, None) => {}
                (Some(linked), Some(parent)) => assert!(Arc::ptr_eq(linked, parent)),
                (linked, parent) => panic!(
                    "block {} is linked to {:?}, but its tracked parent is {:?}",
                    state.hash(),
                    linked.map(|linked| linked.hash()),
                    parent.map(|parent| parent.hash()),
                ),
            }
        }

        assert!(pending_block.is_none_or(|hash| blocks.contains(&hash)));
    }

    #[test]
    fn every_operation_keeps_sections_and_indexes_consistent() {
        let mut builder = TestBlockBuilder::eth();
        let state = CanonicalInMemoryState::empty();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks[1..] {
            state.insert_pending(block.clone());
        }
        assert_consistent(&state);

        // Block 1 arrives last, e.g. loaded from the database, and has to be linked to its
        // already tracked children.
        state.update_chain(NewCanonicalChain::Commit { new: blocks[..3].to_vec() });
        assert_consistent(&state);
        assert_eq!(state.executed_state_by_hash(hash(&blocks[4])).unwrap().chain().count(), 5);

        let fork = builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let fork_child = builder.get_executed_block_with_number(4, hash(&fork));
        state.insert_pending(fork.clone());
        state.set_pending_block(fork_child.clone());
        assert_consistent(&state);

        state.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork.clone(), fork_child.clone()],
            old: vec![blocks[2].clone()],
        });
        assert_consistent(&state);
        state.update_chain(NewCanonicalChain::Reorg {
            new: blocks[2..].to_vec(),
            old: vec![fork, fork_child],
        });
        assert_consistent(&state);
        assert_eq!(state.canonical_block_count(), 5);
        assert_eq!(state.pending_block_count(), 2);

        // The pending block can be a canonical block, and is cleared once that block is trimmed.
        state.set_pending_block(blocks[0].clone());
        let persisted = blocks[1].recovered_block().num_hash();
        state.remove_canonical_blocks_until(persisted.hash, persisted.number);
        assert_consistent(&state);
        assert!(state.pending_state().is_none());
        state.insert_pending(blocks[0].clone());
        assert!(state.pending_state().is_none(), "a trimmed pending block must not come back");
        assert_consistent(&state);

        state.prune_pending_below(blocks[2].recovered_block().num_hash());
        assert_consistent(&state);
        assert_eq!(state.pending_block_count(), 0);

        state.set_pending_block(builder.get_executed_block_with_number(6, hash(&blocks[4])));
        state.demote_canonical_chain();
        assert_consistent(&state);
        assert_eq!(state.canonical_block_count(), 0);
        assert_eq!(state.pending_block_count(), 4);

        assert_eq!(state.clear_state().len(), 4);
        assert_consistent(&state);
        assert_eq!(state.pending_block_count(), 0);
    }

    #[test]
    fn executed_blocks_move_between_sections_and_are_relinked_on_trim() {
        let mut builder = TestBlockBuilder::eth();
        let state = CanonicalInMemoryState::empty();

        // Blocks are validated first and made canonical afterwards, the way the engine does it.
        let canonical = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let inserted =
            canonical.iter().map(|block| state.insert_pending(block.clone())).collect::<Vec<_>>();
        assert_eq!(state.pending_block_count(), 3);
        assert!(state.head_state().is_none());
        assert!(Arc::ptr_eq(inserted[2].parent.as_ref().unwrap(), &inserted[1]));

        state.update_chain(NewCanonicalChain::Commit { new: canonical.clone() });
        assert_eq!(state.canonical_block_count(), 3);
        assert_eq!(state.pending_block_count(), 0);
        for (block, before) in canonical.iter().zip(&inserted) {
            let after = state.state_by_hash(hash(block)).unwrap();
            assert!(Arc::ptr_eq(before, &after), "canonicalization must move the state");
        }

        // A fork off block 1 and the pending block on top of the head live in the pending section.
        let fork = builder.get_executed_block_with_number(2, hash(&canonical[0]));
        let fork_state = state.insert_pending(fork.clone());
        let pending = builder.get_executed_block_with_number(4, hash(&canonical[2]));
        state.set_pending_block(pending);
        let pending_state = state.pending_state().unwrap();
        assert!(state.state_by_hash(hash(&fork)).is_none());
        assert!(Arc::ptr_eq(&fork_state, &state.executed_state_by_hash(hash(&fork)).unwrap()));
        assert_eq!(state.pending_children(hash(&canonical[0])).len(), 1);
        assert_eq!(state.blocks_at_number(2).len(), 2);
        assert_eq!(fork_state.chain().count(), 2);
        assert_eq!(pending_state.chain().count(), 4);

        // Persisting up to block 2 re-links every remaining block, canonical or not, so nothing
        // keeps the trimmed prefix alive.
        let trimmed = inserted[..2].iter().map(Arc::downgrade).collect::<Vec<_>>();
        drop((inserted, fork_state, pending_state));
        let persisted = canonical[1].recovered_block().num_hash();
        let removed = state.remove_canonical_blocks_until(persisted.hash, persisted.number);
        assert_eq!(removed, vec![hash(&canonical[0]), hash(&canonical[1])]);
        assert!(trimmed.iter().all(|state| state.upgrade().is_none()), "trimmed blocks are pinned");

        let head = state.head_state().unwrap();
        assert_eq!(head.hash(), hash(&canonical[2]));
        assert!(head.parent.is_none());
        let pending_state = state.pending_state().unwrap();
        assert!(Arc::ptr_eq(pending_state.parent.as_ref().unwrap(), &head));
        // The fork's parent was trimmed, so its chain now starts at the fork itself.
        let fork_state = state.executed_state_by_hash(hash(&fork)).unwrap();
        assert!(fork_state.parent.is_none());
        assert_eq!(fork_state.anchor(), canonical[0].recovered_block().num_hash());
    }

    #[test]
    fn reorg_moves_blocks_between_sections() {
        let mut builder = TestBlockBuilder::eth();
        let state = CanonicalInMemoryState::empty();

        let base = builder.get_executed_block_with_number(1, B256::random());
        let old_tip = builder.get_executed_block_with_number(2, hash(&base));
        state.update_chain(NewCanonicalChain::Commit { new: vec![base.clone(), old_tip.clone()] });
        let old_tip_state = state.state_by_hash(hash(&old_tip)).unwrap();

        let new_tip = builder.get_executed_block_with_number(2, hash(&base));
        let new_tip_state = state.insert_pending(new_tip.clone());
        state.set_pending_block(new_tip.clone());
        state.update_chain(NewCanonicalChain::Reorg {
            new: vec![new_tip.clone()],
            old: vec![old_tip.clone()],
        });

        assert!(Arc::ptr_eq(&new_tip_state, &state.head_state().unwrap()));
        assert!(state.state_by_hash(hash(&old_tip)).is_none());
        // The reorged block stays tracked as a fork candidate, with the same state.
        assert!(Arc::ptr_eq(
            &old_tip_state,
            &state.executed_state_by_hash(hash(&old_tip)).unwrap()
        ));
        assert!(Arc::ptr_eq(&old_tip_state, &state.pending_children(hash(&base))[0]));
        assert!(state.pending_state().is_none(), "a chain update clears the pending block");

        // Reorging back moves the same states back.
        state.update_chain(NewCanonicalChain::Reorg {
            new: vec![old_tip],
            old: vec![new_tip.clone()],
        });
        assert!(Arc::ptr_eq(&old_tip_state, &state.head_state().unwrap()));
        assert!(Arc::ptr_eq(
            &new_tip_state,
            &state.executed_state_by_hash(hash(&new_tip)).unwrap()
        ));
        assert_eq!(state.canonical_block_count(), 2);
        assert_eq!(state.pending_block_count(), 1);
    }

    #[test]
    fn prune_pending_below_removes_blocks_that_cannot_become_canonical() {
        let mut builder = TestBlockBuilder::eth();
        let state = CanonicalInMemoryState::empty();
        let canonical = builder.get_executed_blocks(1..5).collect::<Vec<_>>();
        state.update_chain(NewCanonicalChain::Commit { new: canonical.clone() });

        let fork_a = builder.get_executed_block_with_number(2, hash(&canonical[0]));
        let fork_a_child = builder.get_executed_block_with_number(3, hash(&fork_a));
        let fork_b = builder.get_executed_block_with_number(3, hash(&canonical[1]));
        let fork_b_child = builder.get_executed_block_with_number(4, hash(&fork_b));
        let fork_c = builder.get_executed_block_with_number(4, hash(&canonical[2]));
        let inserted = [&fork_a, &fork_a_child, &fork_b, &fork_b_child, &fork_c]
            .map(|block| Arc::downgrade(&state.insert_pending(block.clone())));
        state.set_pending_block(fork_b_child.clone());

        // With block 3 finalized, only the fork built on it can still become canonical.
        let removed = state.prune_pending_below(canonical[2].recovered_block().num_hash());
        assert_eq!(
            removed.into_iter().collect::<B256Set>(),
            B256Set::from_iter([fork_a, fork_a_child, fork_b, fork_b_child].iter().map(hash))
        );
        assert_eq!(state.pending_block_count(), 1);
        assert!(state.executed_state_by_hash(hash(&fork_c)).is_some());
        assert!(state.pending_state().is_none(), "the pruned pending block is cleared");
        assert!(
            inserted[..4].iter().all(|state| state.upgrade().is_none()),
            "pruned forks are pinned"
        );
        assert_eq!(state.canonical_block_count(), 4, "canonical blocks are trimmed separately");
    }

    #[test]
    fn inserting_a_trimmed_parent_relinks_its_children() {
        let mut builder = TestBlockBuilder::eth();
        let state = CanonicalInMemoryState::empty();
        let blocks = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let persisted = blocks[0].recovered_block().num_hash();
        state.remove_canonical_blocks_until(persisted.hash, persisted.number);
        assert_eq!(state.head_state().unwrap().chain().count(), 2);

        // A reorg can bring a persisted block back into memory.
        let parent = state.insert_pending(blocks[0].clone());
        let lowest = state.state_by_hash(hash(&blocks[1])).unwrap();
        let head = state.head_state().unwrap();
        assert!(Arc::ptr_eq(lowest.parent.as_ref().unwrap(), &parent));
        assert!(Arc::ptr_eq(head.parent.as_ref().unwrap(), &lowest));
        assert_eq!(head.chain().count(), 3);
    }

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    fn create_mock_state_chain(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        num_blocks: u64,
    ) -> Vec<BlockState> {
        let mut chain = Vec::with_capacity(num_blocks as usize);
        let mut parent_hash = B256::random();
        let mut parent_state: Option<BlockState> = None;

        for i in 1..=num_blocks {
            let mut state = create_mock_state(test_block_builder, i, parent_hash);
            if let Some(parent) = parent_state {
                state.parent = Some(Arc::new(parent));
            }
            parent_hash = state.hash();
            parent_state = Some(state.clone());
            chain.push(state);
        }

        chain
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
    fn test_state() {
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
        assert_eq!(state.hash(), block.recovered_block().hash());
        assert_eq!(state.number(), number);
        assert_eq!(state.state_root(), block.recovered_block().state_root);
    }

    #[test]
    fn test_state_receipts() {
        let receipts = vec![vec![Receipt::default()]];
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block =
            test_block_builder.get_executed_block_with_receipts(receipts.clone(), B256::random());

        let state = BlockState::new(block);

        assert_eq!(state.receipts(), receipts.first().unwrap());
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

        let chain =
            NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1.clone()] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );

        // The reorged block is no longer canonical but stays tracked as a fork candidate.
        assert_eq!(state.canonical_block_count(), 1);
        assert_eq!(state.pending_block_count(), 1);
        assert!(state.state_by_hash(block1.recovered_block().hash()).is_none());
        assert!(state.executed_state_by_hash(block1.recovered_block().hash()).is_some());
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

        // Set the pending block on top of the canonical head
        let block3 =
            test_block_builder.get_executed_block_with_number(2, block2.recovered_block().hash());
        state.set_pending_block(block3.clone());

        // Check the pending state
        assert_eq!(
            *state.pending_state().unwrap(),
            BlockState::with_parent(
                block3.clone(),
                Some(Arc::new(BlockState::with_parent(
                    block2,
                    Some(Arc::new(BlockState::new(block1)))
                )))
            )
        );
        // The pending block lives in the pending section
        assert!(Arc::ptr_eq(
            &state.pending_state().unwrap(),
            &state.executed_state_by_hash(block3.recovered_block().hash()).unwrap()
        ));
        assert!(state.state_by_hash(block3.recovered_block().hash()).is_none());

        // Check the pending block
        assert_eq!(state.pending_block().unwrap(), block3.recovered_block().sealed_block().clone());

        // Check the pending block number and hash
        assert_eq!(
            state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 2, hash: block3.recovered_block().hash() }
        );

        // Check the pending header
        assert_eq!(state.pending_header().unwrap(), block3.recovered_block().header().clone());

        // Check the pending sealed header
        assert_eq!(
            state.pending_sealed_header().unwrap(),
            block3.recovered_block().clone_sealed_header()
        );

        // Check the pending block with senders
        assert_eq!(state.pending_recovered_block().unwrap(), block3.recovered_block().clone());

        // Check the pending block and receipts
        assert_eq!(
            state.pending_block_and_receipts().unwrap(),
            (block3.recovered_block().clone(), vec![])
        );
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
    fn test_block_state_parent_blocks() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 4);

        let parents: Vec<_> = chain[3].parent_state_chain().collect();
        assert_eq!(parents.len(), 3);
        assert_eq!(parents[0].block().recovered_block().number, 3);
        assert_eq!(parents[1].block().recovered_block().number, 2);
        assert_eq!(parents[2].block().recovered_block().number, 1);

        let parents: Vec<_> = chain[2].parent_state_chain().collect();
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].block().recovered_block().number, 2);
        assert_eq!(parents[1].block().recovered_block().number, 1);

        assert_eq!(chain[0].parent_state_chain().count(), 0);
    }

    #[test]
    fn test_block_state_single_block_state_chain() {
        let single_block_number = 1;
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let single_block =
            create_mock_state(&mut test_block_builder, single_block_number, B256::random());
        let single_block_hash = single_block.block().recovered_block().hash();

        assert_eq!(single_block.parent_state_chain().count(), 0);

        let block_state_chain = single_block.chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, single_block_number);
        assert_eq!(block_state_chain[0].block().recovered_block().hash(), single_block_hash);
    }

    #[test]
    fn test_block_state_chain() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 3);

        let block_state_chain = chain[2].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 3);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 3);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[2].block().recovered_block().number, 1);

        let block_state_chain = chain[1].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 2);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 1);

        let block_state_chain = chain[0].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 1);
    }

    #[test]
    fn test_to_chain_notification() {
        // Generate 4 blocks
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block1a =
            test_block_builder.get_executed_block_with_number(1, block0.recovered_block.hash());
        let block2 =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());
        let block2a =
            test_block_builder.get_executed_block_with_number(2, block1.recovered_block.hash());

        // Test commit notification
        let chain_commit = NewCanonicalChain::Commit { new: vec![block0.clone(), block1.clone()] };

        // Build expected trie data map
        let mut expected_trie_data = BTreeMap::new();
        expected_trie_data.insert(0, LazyTrieData::ready(block0.trie_data()));
        expected_trie_data.insert(1, LazyTrieData::ready(block1.trie_data()));

        // Build expected execution outcome (first_block matches first block number)
        let commit_execution_outcome = ExecutionOutcome {
            receipts: vec![vec![], vec![]],
            requests: vec![Requests::default(), Requests::default()],
            first_block: 0,
            ..Default::default()
        };

        assert_eq!(
            chain_commit.to_chain_notification(),
            CanonStateNotification::Commit {
                new: Arc::new(Chain::new(
                    vec![block0.recovered_block().clone(), block1.recovered_block().clone()],
                    commit_execution_outcome,
                    expected_trie_data,
                ))
            }
        );

        // Test reorg notification
        let chain_reorg = NewCanonicalChain::Reorg {
            new: vec![block1a.clone(), block2a.clone()],
            old: vec![block1.clone(), block2.clone()],
        };

        // Build expected trie data for old chain
        let mut old_trie_data = BTreeMap::new();
        old_trie_data.insert(1, LazyTrieData::ready(block1.trie_data()));
        old_trie_data.insert(2, LazyTrieData::ready(block2.trie_data()));

        // Build expected trie data for new chain
        let mut new_trie_data = BTreeMap::new();
        new_trie_data.insert(1, LazyTrieData::ready(block1a.trie_data()));
        new_trie_data.insert(2, LazyTrieData::ready(block2a.trie_data()));

        // Build expected execution outcome for reorg chains (first_block matches first block
        // number)
        let reorg_execution_outcome = ExecutionOutcome {
            receipts: vec![vec![], vec![]],
            requests: vec![Requests::default(), Requests::default()],
            first_block: 1,
            ..Default::default()
        };

        assert_eq!(
            chain_reorg.to_chain_notification(),
            CanonStateNotification::Reorg {
                old: Arc::new(Chain::new(
                    vec![block1.recovered_block().clone(), block2.recovered_block().clone()],
                    reorg_execution_outcome.clone(),
                    old_trie_data,
                )),
                new: Arc::new(Chain::new(
                    vec![block1a.recovered_block().clone(), block2a.recovered_block().clone()],
                    reorg_execution_outcome,
                    new_trie_data,
                ))
            }
        );
    }

    #[test]
    fn test_to_chain_notification_carries_prepared_bal() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block0 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block1 = test_block_builder
            .get_executed_block_with_number(1, block0.recovered_block.hash())
            .with_bal(Some(Arc::new(DecodedRevmBal::new(
                Arc::new(revm::state::bal::Bal::default()),
                Bytes::from_static(&[0xc0]),
            ))));

        let chain = NewCanonicalChain::Commit { new: vec![block0, block1.clone()] };
        let CanonStateNotification::Commit { new } = chain.to_chain_notification() else {
            panic!("expected a commit notification")
        };

        // Only the block whose payload carried a BAL contributes one.
        assert_eq!(new.bals().len(), 1);
        assert_eq!(new.bal_at(1), block1.bal());

        let mut blocks_and_bals = new.blocks_and_bals();
        let (block, bal) = blocks_and_bals.next().expect("block with BAL");
        assert_eq!(block.hash(), block1.recovered_block.hash());
        assert_eq!(Some(bal), block1.bal());
        assert!(blocks_and_bals.next().is_none());
    }
}
