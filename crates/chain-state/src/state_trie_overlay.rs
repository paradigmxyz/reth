//! Flattened state trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`StateTrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened state trie overlays on demand.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::B256;
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use reth_primitives_traits::{
    dashmap::{mapref::entry::Entry, DashMap},
    AlloyBlockHeader, NodePrimitives,
};
#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted};
#[cfg(any(test, feature = "rayon"))]
use std::time::Instant;
use std::{fmt, sync::Arc};
use tracing::debug;

/// Manages flattened state trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph and a cache of flattened state trie overlays keyed by
/// `(anchor_hash, tip_hash)`. Cache entries can also mark in-flight background computations.
#[derive(Clone)]
pub struct StateTrieOverlayManager<N: NodePrimitives = EthPrimitives> {
    blocks: Arc<DashMap<B256, ExecutedBlock<N>>>,
    overlays: Arc<DashMap<OverlayCacheKey, OverlayCacheEntry>>,
    #[cfg(feature = "rayon")]
    worker_pool: Option<Arc<WorkerPool>>,
    metrics: StateTrieOverlayMetrics,
}

/// Metrics for state trie overlay management.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.block_validation.state_trie_overlay")]
struct StateTrieOverlayMetrics {
    /// Duration of overlay computation in seconds.
    overlay_computation_duration_seconds: Histogram,
    /// Number of requests satisfied by an existing overlay cache entry.
    overlay_cache_reuses: Counter,
    /// Number of overlay cache entries populated by computing an overlay.
    overlay_cache_fills: Counter,
}

impl<N: NodePrimitives> Default for StateTrieOverlayManager<N> {
    fn default() -> Self {
        Self {
            blocks: Default::default(),
            overlays: Default::default(),
            #[cfg(feature = "rayon")]
            worker_pool: None,
            metrics: Default::default(),
        }
    }
}

impl<N: NodePrimitives> std::fmt::Debug for StateTrieOverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateTrieOverlayManager")
            .field("blocks", &self.blocks.len())
            .field("overlays", &self.overlays.len())
            .finish()
    }
}

impl<N: NodePrimitives> StateTrieOverlayManager<N> {
    /// Create a new [`StateTrieOverlayManager`] backed by the given worker pool.
    #[cfg(feature = "rayon")]
    pub fn new(worker_pool: Arc<WorkerPool>) -> Self {
        Self {
            blocks: Default::default(),
            overlays: Default::default(),
            worker_pool: Some(worker_pool),
            metrics: Default::default(),
        }
    }

    /// Inserts an executed in-memory block into the state trie overlay manager.
    #[tracing::instrument(
        level = "trace",
        target = "chain_state::state_trie_overlay",
        skip_all,
        fields(
            block_hash = %block.recovered_block().hash(),
            parent_hash = %block.recovered_block().parent_hash(),
            duplicate = false,
        )
    )]
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let hash = block.recovered_block().hash();
        let parent_hash = block.recovered_block().parent_hash();
        let span = tracing::Span::current();

        // First add the block to the live graph; duplicate inserts do not need cache work.
        match self.blocks.entry(hash) {
            Entry::Occupied(_) => {
                span.record("duplicate", true);
                debug!(
                    target: "chain_state::state_trie_overlay",
                    %hash,
                    %parent_hash,
                    "state trie overlay block already inserted"
                );
                return
            }
            Entry::Vacant(entry) => {
                entry.insert(block);
            }
        }

        // Snapshot matching parent overlays before spawning so DashMap iteration guards are
        // dropped.
        let cached_parent_overlays = self
            .overlays
            .iter()
            .filter_map(|entry| {
                let key = *entry.key();
                (key.tip_hash == parent_hash && entry.value().is_ready()).then_some(key.anchor_hash)
            })
            .collect::<Vec<_>>();

        debug!(
            target: "chain_state::state_trie_overlay",
            %hash,
            %parent_hash,
            "inserted block into state trie overlay manager"
        );
        if cached_parent_overlays.is_empty() {
            return
        }

        let _guard = span.enter();
        for anchor_hash in cached_parent_overlays {
            self.spawn_overlay_cache_fill(OverlayCacheKey { anchor_hash, tip_hash: hash });
        }
    }

    /// Removes blocks from the live block graph and prunes cached overlays that can no longer be
    /// built from the remaining blocks.
    #[tracing::instrument(
        level = "trace",
        target = "chain_state::state_trie_overlay",
        skip_all,
        fields(
            block_count = tracing::field::Empty,
            removed_blocks = tracing::field::Empty,
            pruned_overlays = tracing::field::Empty,
        )
    )]
    pub fn remove_blocks(&self, hashes: impl IntoIterator<Item = B256>) {
        let span = tracing::Span::current();

        // Remove blocks first, then prune overlays against the remaining block graph.
        let mut block_count = 0usize;
        let mut removed_blocks = 0usize;
        let mut pruned_overlays = 0usize;
        for hash in hashes {
            block_count += 1;
            removed_blocks += self.blocks.remove(&hash).is_some() as usize;
        }
        span.record("block_count", block_count);
        span.record("removed_blocks", removed_blocks);

        if removed_blocks > 0 {
            let overlays_before = self.overlays.len();
            let blocks = Arc::clone(&self.blocks);
            self.overlays.retain(|key, _| {
                key.tip_hash != key.anchor_hash &&
                    Self::anchor_for_parent_in(blocks.as_ref(), key.tip_hash, key.anchor_hash) ==
                        Some(key.anchor_hash)
            });
            pruned_overlays = overlays_before.saturating_sub(self.overlays.len());
            span.record("pruned_overlays", pruned_overlays);
        }
        debug!(
            target: "chain_state::state_trie_overlay",
            block_count,
            removed_blocks,
            pruned_overlays,
            "removed blocks from state trie overlay manager"
        );
    }

    /// Returns the flattened overlay from `anchor_hash` to `parent_hash`.
    #[tracing::instrument(
        level = "trace",
        target = "chain_state::state_trie_overlay",
        skip_all,
        fields(tip_hash = %parent_hash, anchor_hash = %anchor_hash)
    )]
    pub fn overlay_for_parent(
        &self,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<StateTrieOverlay, StateTrieOverlayError> {
        debug!(
            target: "chain_state::state_trie_overlay",
            tip_hash = %parent_hash,
            %anchor_hash,
            "loading state trie overlay for parent"
        );
        let overlay = self.get_overlay(parent_hash, anchor_hash)?;
        Ok(overlay)
    }

    #[tracing::instrument(
        level = "trace",
        target = "chain_state::state_trie_overlay",
        skip_all,
        fields(
            tip_hash = %tip_hash,
            anchor_hash = %anchor_hash,
            cache_reused = tracing::field::Empty,
            block_count = tracing::field::Empty,
            parent_overlay_reused = tracing::field::Empty,
        )
    )]
    fn get_overlay(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<StateTrieOverlay, StateTrieOverlayError> {
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();

        if let Some(overlay) = self.ready_overlay(key) {
            self.metrics.overlay_cache_reuses.increment(1);
            span.record("cache_reused", true);
            return Ok(overlay)
        }
        span.record("cache_reused", false);

        let blocks = self.resolve_block_path(tip_hash, anchor_hash)?;
        span.record("block_count", blocks.len());
        if blocks.is_empty() {
            return Ok(StateTrieOverlay::default())
        }

        let cached_prefix = self.largest_cached_prefix(anchor_hash, &blocks);
        span.record("parent_overlay_reused", cached_prefix.is_some());

        self.spawn_overlay_cache_fill(key);

        Ok(Self::overlay_stack_from_path(&blocks, cached_prefix))
    }

    fn resolve_block_path(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<Vec<ExecutedBlock<N>>, StateTrieOverlayError> {
        if tip_hash == anchor_hash {
            return Ok(Vec::new())
        }

        let mut hash = tip_hash;
        let mut blocks = Vec::new();
        loop {
            let block =
                self.blocks.get(&hash).ok_or(StateTrieOverlayError { tip_hash, anchor_hash })?;
            let parent_hash = block.recovered_block().parent_hash();
            blocks.push(block.clone());

            if parent_hash == anchor_hash {
                break
            }
            hash = parent_hash;
        }
        Ok(blocks)
    }

    fn largest_cached_prefix(
        &self,
        anchor_hash: B256,
        blocks_newest_to_oldest: &[ExecutedBlock<N>],
    ) -> Option<(usize, StateTrieOverlay)> {
        blocks_newest_to_oldest.iter().enumerate().find_map(|(idx, block)| {
            let tip_hash = block.recovered_block().hash();
            self.ready_overlay(OverlayCacheKey { anchor_hash, tip_hash })
                .map(|overlay| (idx, overlay))
        })
    }

    fn overlay_stack_from_path(
        blocks_newest_to_oldest: &[ExecutedBlock<N>],
        cached_prefix: Option<(usize, StateTrieOverlay)>,
    ) -> StateTrieOverlay {
        let individual_block_count =
            cached_prefix.as_ref().map_or(blocks_newest_to_oldest.len(), |(idx, _)| *idx);
        let mut trie_updates =
            Vec::with_capacity(individual_block_count + cached_prefix.is_some() as usize);
        let mut hashed_post_state =
            Vec::with_capacity(individual_block_count + cached_prefix.is_some() as usize);

        for block in &blocks_newest_to_oldest[..individual_block_count] {
            let trie_data = block.trie_data();
            trie_updates.push(trie_data.trie_updates);
            hashed_post_state.push(trie_data.hashed_state);
        }

        if let Some((_, cached_overlay)) = cached_prefix {
            trie_updates.extend(cached_overlay.trie_updates);
            hashed_post_state.extend(cached_overlay.hashed_post_state);
        }

        StateTrieOverlay::new(trie_updates, hashed_post_state)
    }

    fn spawn_overlay_cache_fill(&self, key: OverlayCacheKey) {
        #[cfg(not(feature = "rayon"))]
        {
            let _ = key;
        }

        #[cfg(feature = "rayon")]
        {
            let Some(worker_pool) = self.worker_pool.clone() else { return };

            match self.overlays.entry(key) {
                Entry::Occupied(_) => return,
                Entry::Vacant(entry) => {
                    entry.insert(OverlayCacheEntry::Pending);
                }
            }

            let manager = <Self as Clone>::clone(self);
            let span = tracing::Span::current();

            worker_pool.spawn(move || {
                let _span = tracing::debug_span!(
                    target: "chain_state::state_trie_overlay",
                    parent: span,
                    "compute_state_trie_overlay_cache_fill",
                    tip_hash = %key.tip_hash,
                    anchor_hash = %key.anchor_hash,
                )
                .entered();
                manager.compute_and_cache_overlay(key);
            });
        }
    }

    #[cfg(any(test, feature = "rayon"))]
    fn compute_and_cache_overlay(&self, key: OverlayCacheKey) {
        let result = self.compute_overlay_for_key(key);

        if let Err(error) = result {
            self.remove_pending_overlay(key);
            debug!(
                target: "chain_state::state_trie_overlay",
                ?error,
                tip_hash = %key.tip_hash,
                anchor_hash = %key.anchor_hash,
                "failed to compute state trie overlay cache fill"
            );
        }
    }

    #[cfg(any(test, feature = "rayon"))]
    fn compute_overlay_for_key(
        &self,
        key: OverlayCacheKey,
    ) -> Result<StateTrieOverlay, StateTrieOverlayError> {
        let blocks = self.resolve_block_path(key.tip_hash, key.anchor_hash)?;
        let cached_prefix = self.largest_cached_prefix(key.anchor_hash, &blocks);
        let (blocks, parent_overlay) = match cached_prefix {
            Some((idx, parent_overlay)) => (blocks[..idx].to_vec(), parent_overlay),
            None => (blocks, StateTrieOverlay::default()),
        };
        let overlay = compute_overlay(blocks, parent_overlay, key.anchor_hash, &self.metrics);

        if key.tip_hash != key.anchor_hash &&
            Self::anchor_for_parent_in(self.blocks.as_ref(), key.tip_hash, key.anchor_hash) !=
                Some(key.anchor_hash)
        {
            return Err(StateTrieOverlayError {
                tip_hash: key.tip_hash,
                anchor_hash: key.anchor_hash,
            });
        }

        let overlay = match self.overlays.entry(key) {
            Entry::Occupied(mut entry) => match entry.get() {
                OverlayCacheEntry::Ready(overlay) => {
                    self.metrics.overlay_cache_reuses.increment(1);
                    overlay.clone()
                }
                OverlayCacheEntry::Pending => {
                    self.metrics.overlay_cache_fills.increment(1);
                    entry.insert(OverlayCacheEntry::Ready(overlay.clone()));
                    overlay
                }
            },
            Entry::Vacant(entry) => {
                self.metrics.overlay_cache_fills.increment(1);
                entry.insert(OverlayCacheEntry::Ready(overlay.clone()));
                overlay
            }
        };

        Ok(overlay)
    }

    fn ready_overlay(&self, key: OverlayCacheKey) -> Option<StateTrieOverlay> {
        self.overlays.get(&key).and_then(|entry| entry.value().ready())
    }

    #[cfg(any(test, feature = "rayon"))]
    fn remove_pending_overlay(&self, key: OverlayCacheKey) {
        self.overlays.remove_if(&key, |_, entry| matches!(entry, OverlayCacheEntry::Pending));
    }

    /// Returns `preferred_anchor` if it is on the parent chain, otherwise the first missing parent.
    ///
    /// Returns `None` if `parent_hash` is not `preferred_anchor` and the manager does not contain a
    /// block for `parent_hash`, meaning there is no in-memory parent chain to inspect.
    pub fn anchor_for_parent(&self, parent_hash: B256, preferred_anchor: B256) -> Option<B256> {
        Self::anchor_for_parent_in(self.blocks.as_ref(), parent_hash, preferred_anchor)
    }

    fn anchor_for_parent_in(
        blocks: &DashMap<B256, ExecutedBlock<N>>,
        parent_hash: B256,
        preferred_anchor: B256,
    ) -> Option<B256> {
        if parent_hash == preferred_anchor {
            return Some(preferred_anchor)
        }

        let mut hash = parent_hash;

        loop {
            let block_parent_hash = blocks.get(&hash)?.recovered_block().parent_hash();
            if block_parent_hash == preferred_anchor {
                return Some(block_parent_hash)
            }
            if !blocks.contains_key(&block_parent_hash) {
                return Some(block_parent_hash)
            }
            hash = block_parent_hash;
        }
    }
}

/// State trie overlays ordered from highest to lowest precedence.
#[derive(Clone, Debug, Default)]
pub struct StateTrieOverlay {
    /// Trie updates overlays.
    pub trie_updates: Vec<Arc<TrieUpdatesSorted>>,
    /// Hashed post state overlays.
    pub hashed_post_state: Vec<Arc<HashedPostStateSorted>>,
}

impl StateTrieOverlay {
    /// Create a new state trie overlay.
    pub const fn new(
        trie_updates: Vec<Arc<TrieUpdatesSorted>>,
        hashed_post_state: Vec<Arc<HashedPostStateSorted>>,
    ) -> Self {
        Self { trie_updates, hashed_post_state }
    }
}

/// Error returned when a state trie overlay cannot be built from the manager's current block set.
#[derive(Debug)]
pub struct StateTrieOverlayError {
    /// Requested in-memory tip hash.
    tip_hash: B256,
    /// Requested anchor hash.
    anchor_hash: B256,
}

impl fmt::Display for StateTrieOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "state trie overlay for tip {} cannot be anchored to {} with current blocks",
            self.tip_hash, self.anchor_hash
        )
    }
}

impl std::error::Error for StateTrieOverlayError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct OverlayCacheKey {
    anchor_hash: B256,
    tip_hash: B256,
}

#[cfg_attr(not(any(test, feature = "rayon")), allow(dead_code))]
enum OverlayCacheEntry {
    /// An in-flight background cache fill.
    ///
    /// Read paths treat this as a cache miss so they can still return a lazy overlay stack.
    Pending,
    /// A flattened overlay ready for reuse.
    Ready(StateTrieOverlay),
}

impl OverlayCacheEntry {
    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    fn ready(&self) -> Option<StateTrieOverlay> {
        match self {
            Self::Pending => None,
            Self::Ready(overlay) => Some(overlay.clone()),
        }
    }
}

#[tracing::instrument(
    level = "trace",
    target = "chain_state::state_trie_overlay",
    skip_all,
    fields(
        anchor_hash = %anchor_hash,
        block_count = tracing::field::Empty,
        parent_overlay = tracing::field::Empty,
        elapsed_us = tracing::field::Empty,
    )
)]
#[cfg(any(test, feature = "rayon"))]
fn compute_overlay<N: NodePrimitives>(
    blocks: Vec<ExecutedBlock<N>>,
    parent_overlay: StateTrieOverlay,
    anchor_hash: B256,
    metrics: &StateTrieOverlayMetrics,
) -> StateTrieOverlay {
    let started_at = Instant::now();
    let block_count = blocks.len();
    let parent_overlay_reused =
        !parent_overlay.trie_updates.is_empty() || !parent_overlay.hashed_post_state.is_empty();
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay_reused);

    let overlay = flatten_overlay(blocks, parent_overlay);

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "chain_state::state_trie_overlay",
        %anchor_hash,
        block_count,
        parent_overlay = parent_overlay_reused,
        ?elapsed,
        "computed state trie overlay"
    );

    overlay
}

#[cfg(any(test, feature = "rayon"))]
fn flatten_overlay<N: NodePrimitives>(
    blocks: Vec<ExecutedBlock<N>>,
    parent_overlay: StateTrieOverlay,
) -> StateTrieOverlay {
    let trie_data = blocks.iter().map(ExecutedBlock::trie_data).collect::<Vec<_>>();
    let StateTrieOverlay { trie_updates: parent_trie_updates, hashed_post_state } = parent_overlay;

    #[cfg(feature = "rayon")]
    let (trie_updates, hashed_post_state) = rayon::join(
        || {
            TrieUpdatesSorted::merge_batch(
                trie_data
                    .iter()
                    .map(|data| Arc::clone(&data.trie_updates))
                    .chain(parent_trie_updates),
            )
        },
        || {
            HashedPostStateSorted::merge_batch(
                trie_data
                    .iter()
                    .map(|data| Arc::clone(&data.hashed_state))
                    .chain(hashed_post_state),
            )
        },
    );

    #[cfg(not(feature = "rayon"))]
    let (trie_updates, hashed_post_state) = (
        TrieUpdatesSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.trie_updates)).chain(parent_trie_updates),
        ),
        HashedPostStateSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.hashed_state)).chain(hashed_post_state),
        ),
    );

    StateTrieOverlay::new(vec![trie_updates], vec![hashed_post_state])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, ComputedTrieData, EthPrimitives, ExecutedBlock};
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage};
    use std::sync::Arc;
    #[cfg(feature = "rayon")]
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn with_unique_state(
        block: &ExecutedBlock<EthPrimitives>,
        id: u8,
    ) -> ExecutedBlock<EthPrimitives> {
        let hashed_address = B256::with_last_byte(id);
        let hashed_slot = B256::with_last_byte(id.saturating_add(32));
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(TrieUpdatesSorted::default())),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..4)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    fn state_account_count(states: &[Arc<HashedPostStateSorted>]) -> usize {
        states.iter().map(|state| state.accounts.len()).sum()
    }

    #[test]
    fn errors_for_unknown_parent() {
        let manager = StateTrieOverlayManager::<EthPrimitives>::default();
        let parent = B256::random();
        let anchor = B256::random();

        let err = manager.overlay_for_parent(parent, anchor).unwrap_err();

        assert_eq!(err.tip_hash, parent);
        assert_eq!(err.anchor_hash, anchor);
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();

        let state = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap()
            .hashed_post_state;
        assert_eq!(state.len(), 3);
        assert_eq!(state_account_count(&state), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let short = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor)
            .unwrap()
            .hashed_post_state;
        assert_eq!(short.len(), 1);
        assert_eq!(state_account_count(&short), 1);
        manager.compute_and_cache_overlay(OverlayCacheKey {
            anchor_hash: short_anchor,
            tip_hash: blocks[2].recovered_block().hash(),
        });
        let cached_short = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor)
            .unwrap()
            .hashed_post_state;
        assert_eq!(cached_short.len(), 1);
        assert_eq!(cached_short[0].accounts.len(), 1);
    }

    #[test]
    fn cache_miss_returns_largest_cached_prefix_and_individual_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let prefix_tip = blocks[1].recovered_block().hash();
        manager.compute_and_cache_overlay(OverlayCacheKey { anchor_hash, tip_hash: prefix_tip });

        let state = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap()
            .hashed_post_state;
        assert_eq!(state.len(), 2);
        assert_eq!(state[0].accounts.len(), 1);
        assert_eq!(state[1].accounts.len(), 2);
        assert_eq!(state_account_count(&state), 3);
    }

    #[test]
    fn pending_overlay_entries_are_ignored_by_read_path() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let prefix_tip = blocks[1].recovered_block().hash();
        let prefix_key = OverlayCacheKey { anchor_hash, tip_hash: prefix_tip };
        manager.overlays.insert(prefix_key, OverlayCacheEntry::Pending);

        let state = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap()
            .hashed_post_state;
        assert_eq!(state.len(), 3);
        assert_eq!(state_account_count(&state), 3);
        assert!(matches!(
            manager.overlays.get(&prefix_key).as_deref(),
            Some(OverlayCacheEntry::Pending)
        ));
    }

    #[test]
    fn returns_anchor_for_in_memory_parent() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        assert_eq!(
            manager.anchor_for_parent(blocks[2].recovered_block().hash(), B256::random()),
            Some(blocks[0].recovered_block().parent_hash())
        );

        manager.remove_blocks([blocks[0].recovered_block().hash()]);
        assert_eq!(
            manager.anchor_for_parent(
                blocks[2].recovered_block().hash(),
                blocks[0].recovered_block().hash()
            ),
            Some(blocks[0].recovered_block().hash())
        );
    }

    #[test]
    fn prefers_anchor_in_parent_chain() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let db_tip_hash = blocks[1].recovered_block().hash();
        assert_eq!(
            manager.anchor_for_parent(blocks[2].recovered_block().hash(), db_tip_hash),
            Some(db_tip_hash)
        );
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn insert_block_prepares_child_overlay_from_cached_parent() {
        let manager = StateTrieOverlayManager::new(Arc::new(WorkerPool::new(2, "test-ovly")));
        let blocks = test_blocks();

        manager.insert_block(blocks[0].clone());

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[0].recovered_block().hash();
        manager.compute_and_cache_overlay(OverlayCacheKey { anchor_hash, tip_hash: parent_hash });

        let child_hash = blocks[1].recovered_block().hash();
        manager.insert_block(blocks[1].clone());

        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.ready_overlay(child_key).is_none() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for optimistically prepared child overlay"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let state = manager.overlay_for_parent(child_hash, anchor_hash).unwrap().hashed_post_state;
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].accounts.len(), 2);
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn insert_block_respects_pending_child_overlay_fill() {
        let manager = StateTrieOverlayManager::new(Arc::new(WorkerPool::new(2, "test-ovly")));
        let blocks = test_blocks();

        manager.insert_block(blocks[0].clone());

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[0].recovered_block().hash();
        manager.compute_and_cache_overlay(OverlayCacheKey { anchor_hash, tip_hash: parent_hash });

        let child_hash = blocks[1].recovered_block().hash();
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        manager.overlays.insert(child_key, OverlayCacheEntry::Pending);

        manager.insert_block(blocks[1].clone());
        thread::sleep(Duration::from_millis(100));

        assert!(manager.ready_overlay(child_key).is_none());
        assert!(matches!(
            manager.overlays.get(&child_key).as_deref(),
            Some(OverlayCacheEntry::Pending)
        ));
        manager.overlays.remove(&child_key);
    }

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let original_anchor = blocks[0].recovered_block().parent_hash();
        manager.compute_and_cache_overlay(OverlayCacheKey {
            anchor_hash: original_anchor,
            tip_hash: blocks[2].recovered_block().hash(),
        });

        manager.remove_blocks([
            blocks[0].recovered_block().hash(),
            blocks[1].recovered_block().hash(),
        ]);

        let anchor_hash = blocks[1].recovered_block().hash();
        assert!(manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .is_err());

        let state = manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap()
            .hashed_post_state;
        assert_eq!(state_account_count(&state), 1);
    }
}
