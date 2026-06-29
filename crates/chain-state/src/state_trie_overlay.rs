//! Flattened state trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`StateTrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened state trie overlays on demand.

use crate::{EthPrimitives, ExecutedBlock, PreservedSparseTrie, PreservedTrieGuard};
use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use reth_primitives_traits::{
    dashmap::{mapref::entry::Entry, DashMap},
    AlloyBlockHeader, FastInstant, NodePrimitives,
};
#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{
    fmt,
    sync::{Arc, OnceLock},
    time::Instant,
};
use tracing::{debug, trace};

/// Manages flattened state trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph and a cache of flattened state trie overlays keyed by
/// `(anchor_hash, tip_hash)`.
#[derive(Clone)]
pub struct StateTrieOverlayManager<N: NodePrimitives = EthPrimitives> {
    blocks: Arc<DashMap<B256, ExecutedBlock<N>>>,
    overlays: Arc<DashMap<OverlayCacheKey, OverlayCacheEntry>>,
    preserved_sparse_trie: Arc<Mutex<Option<PreservedSparseTrie>>>,
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
            preserved_sparse_trie: Default::default(),
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
            preserved_sparse_trie: Default::default(),
            worker_pool: Some(worker_pool),
            metrics: Default::default(),
        }
    }

    /// Takes the preserved sparse trie if present, leaving `None` in its place.
    pub fn take_sparse_trie(&self) -> Option<PreservedSparseTrie> {
        self.preserved_sparse_trie.lock().take()
    }

    /// Acquires a guard that blocks taking the trie until dropped.
    ///
    /// Use this before sending the state root result to ensure the next block waits for the trie
    /// to be stored.
    pub fn lock_sparse_trie(&self) -> PreservedTrieGuard<'_> {
        PreservedTrieGuard::new(self.preserved_sparse_trie.lock())
    }

    /// Waits until the sparse trie lock becomes available.
    ///
    /// This acquires and immediately releases the lock, ensuring that any ongoing operations
    /// complete before returning. Returns the time spent waiting for the lock.
    pub fn wait_for_sparse_trie_availability(&self) -> std::time::Duration {
        let start = FastInstant::now();
        let _guard = self.preserved_sparse_trie.lock();
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "engine::tree::payload_processor",
                blocked_for=?elapsed,
                "Waited for preserved sparse trie to become available"
            );
        }
        elapsed
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

        #[cfg(feature = "rayon")]
        let Some(worker_pool) = self.worker_pool.clone() else {
            return
        };

        #[cfg(not(feature = "rayon"))]
        let _ = cached_parent_overlays;

        #[cfg(feature = "rayon")]
        {
            let parent_span = span;
            for anchor_hash in cached_parent_overlays {
                let manager = <Self as Clone>::clone(self);
                let parent_span = parent_span.clone();
                worker_pool.spawn(move || {
                    let _span = tracing::trace_span!(
                        target: "chain_state::state_trie_overlay",
                        parent: parent_span,
                        "precompute_state_trie_overlay",
                        tip_hash = %hash,
                        anchor_hash = %anchor_hash,
                    )
                    .entered();
                    let _ = manager.precompute_overlay(hash, anchor_hash);
                });
            }
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
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        debug!(
            target: "chain_state::state_trie_overlay",
            tip_hash = %parent_hash,
            %anchor_hash,
            "loading state trie overlay for parent"
        );
        let input = self.get_overlay(parent_hash, anchor_hash)?;
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    #[cfg(feature = "rayon")]
    fn precompute_overlay(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<(), StateTrieOverlayError> {
        let _ = self.get_overlay_inner(tip_hash, anchor_hash, OverlayLookupMode::Precompute)?;
        Ok(())
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
    ) -> Result<Arc<TrieInputSorted>, StateTrieOverlayError> {
        self.get_overlay_inner(tip_hash, anchor_hash, OverlayLookupMode::Required)
            .map(|input| input.expect("required overlay lookups always return an overlay"))
    }

    fn get_overlay_inner(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
        mode: OverlayLookupMode,
    ) -> Result<Option<Arc<TrieInputSorted>>, StateTrieOverlayError> {
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();

        if let Some(entry) = self.overlays.get(&key).map(|entry| entry.value().clone()) {
            return Ok(match entry {
                OverlayCacheEntry::Ready(input) => {
                    self.metrics.overlay_cache_reuses.increment(1);
                    span.record("cache_reused", true);
                    Some(input)
                }
                OverlayCacheEntry::Computing(waiter) => {
                    span.record("cache_reused", true);
                    if mode == OverlayLookupMode::Precompute {
                        None
                    } else {
                        self.metrics.overlay_cache_reuses.increment(1);
                        Some(waiter.wait())
                    }
                }
            })
        }
        span.record("cache_reused", false);

        // Resolve the block path and any cached parent overlay before locking the child entry.
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
        span.record("block_count", blocks.len());
        let parent_input = blocks.first().and_then(|block| {
            let parent_hash = block.recovered_block().parent_hash();
            (parent_hash != anchor_hash)
                .then(|| {
                    self.overlays
                        .get(&OverlayCacheKey { anchor_hash, tip_hash: parent_hash })
                        .and_then(|entry| entry.value().ready())
                })
                .flatten()
        });
        span.record("parent_overlay_reused", parent_input.is_some());
        let compute_input = match parent_input {
            Some(parent_input) => {
                ComputeOverlayInput::ExtendCached { block: blocks.swap_remove(0), parent_input }
            }
            None => ComputeOverlayInput::MergeBlocks(blocks),
        };

        enum CacheAction {
            Ready(Arc<TrieInputSorted>),
            Wait(Arc<OverlayWaiter>),
            Compute(Arc<OverlayWaiter>),
            Skip,
        }

        let action = match self.overlays.entry(key) {
            Entry::Occupied(entry) => match entry.get().clone() {
                OverlayCacheEntry::Ready(input) => {
                    self.metrics.overlay_cache_reuses.increment(1);
                    span.record("cache_reused", true);
                    CacheAction::Ready(input)
                }
                OverlayCacheEntry::Computing(waiter) => {
                    span.record("cache_reused", true);
                    if mode == OverlayLookupMode::Precompute {
                        CacheAction::Skip
                    } else {
                        self.metrics.overlay_cache_reuses.increment(1);
                        CacheAction::Wait(waiter)
                    }
                }
            },
            Entry::Vacant(entry) => {
                self.metrics.overlay_cache_fills.increment(1);
                let waiter = Arc::new(OverlayWaiter::new());
                entry.insert(OverlayCacheEntry::Computing(Arc::clone(&waiter)));
                CacheAction::Compute(waiter)
            }
        };

        match action {
            CacheAction::Ready(input) => Ok(Some(input)),
            CacheAction::Wait(waiter) => Ok(Some(waiter.wait())),
            CacheAction::Skip => Ok(None),
            CacheAction::Compute(waiter) => {
                let input = self.compute_overlay(compute_input, anchor_hash, span);
                waiter.finish(Arc::clone(&input));

                if let Entry::Occupied(mut entry) = self.overlays.entry(key) {
                    // The entry may have been pruned while the overlay was computing. Only cache
                    // the result if the map still points at the waiter installed by this task.
                    let should_publish = match entry.get() {
                        OverlayCacheEntry::Computing(existing) => Arc::ptr_eq(existing, &waiter),
                        OverlayCacheEntry::Ready(_) => false,
                    };
                    if should_publish {
                        entry.insert(OverlayCacheEntry::Ready(Arc::clone(&input)));
                    }
                }

                Ok(Some(input))
            }
        }
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

    fn compute_overlay(
        &self,
        compute_input: ComputeOverlayInput<N>,
        anchor_hash: B256,
        _span: tracing::Span,
    ) -> Arc<TrieInputSorted> {
        #[cfg(feature = "rayon")]
        {
            if let Some(worker_pool) = &self.worker_pool {
                let compute_span = _span;
                let metrics = self.metrics.clone();
                return Arc::new(worker_pool.install_fn(move || {
                    let _guard = compute_span.enter();
                    compute_overlay(compute_input, anchor_hash, &metrics)
                }))
            }
        }

        Arc::new(compute_overlay(compute_input, anchor_hash, &self.metrics))
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

#[derive(Clone)]
enum OverlayCacheEntry {
    Ready(Arc<TrieInputSorted>),
    Computing(Arc<OverlayWaiter>),
}

impl OverlayCacheEntry {
    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    fn ready(&self) -> Option<Arc<TrieInputSorted>> {
        match self {
            Self::Ready(input) => Some(Arc::clone(input)),
            Self::Computing(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayLookupMode {
    Required,
    Precompute,
}

struct OverlayWaiter {
    input: OnceLock<Arc<TrieInputSorted>>,
}

impl OverlayWaiter {
    const fn new() -> Self {
        Self { input: OnceLock::new() }
    }

    fn wait(&self) -> Arc<TrieInputSorted> {
        Arc::clone(self.input.wait())
    }

    fn finish(&self, computed: Arc<TrieInputSorted>) {
        let _ = self.input.set(computed);
    }
}

enum ComputeOverlayInput<N: NodePrimitives> {
    ExtendCached { block: ExecutedBlock<N>, parent_input: Arc<TrieInputSorted> },
    MergeBlocks(Vec<ExecutedBlock<N>>),
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
fn compute_overlay<N: NodePrimitives>(
    input: ComputeOverlayInput<N>,
    anchor_hash: B256,
    metrics: &StateTrieOverlayMetrics,
) -> TrieInputSorted {
    let started_at = Instant::now();
    let block_count = match &input {
        ComputeOverlayInput::ExtendCached { .. } => 1,
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { block, parent_input } => {
            let trie_data = block.trie_data();

            trace!(
                target: "chain_state::state_trie_overlay",
                %anchor_hash,
                head = %block.recovered_block().hash(),
                "extending cached parent state trie overlay"
            );

            let mut overlay = parent_input.as_ref().clone();
            extend_overlay(&mut overlay, &trie_data.hashed_state, &trie_data.trie_updates);
            overlay
        }
        ComputeOverlayInput::MergeBlocks(blocks) => merge_blocks(blocks),
    };

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "chain_state::state_trie_overlay",
        %anchor_hash,
        block_count,
        parent_overlay,
        ?elapsed,
        "computed state trie overlay"
    );

    overlay
}

fn merge_blocks<N: NodePrimitives>(blocks: Vec<ExecutedBlock<N>>) -> TrieInputSorted {
    let trie_data = blocks.iter().map(ExecutedBlock::trie_data).collect::<Vec<_>>();

    #[cfg(feature = "rayon")]
    let (nodes, state) = rayon::join(
        || {
            TrieUpdatesSorted::merge_batch(
                trie_data.iter().map(|data| Arc::clone(&data.trie_updates)),
            )
        },
        || {
            HashedPostStateSorted::merge_batch(
                trie_data.iter().map(|data| Arc::clone(&data.hashed_state)),
            )
        },
    );

    #[cfg(not(feature = "rayon"))]
    let (nodes, state) = (
        TrieUpdatesSorted::merge_batch(trie_data.iter().map(|data| Arc::clone(&data.trie_updates))),
        HashedPostStateSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.hashed_state)),
        ),
    );

    TrieInputSorted::new(nodes, state, Default::default())
}

fn extend_overlay(
    overlay: &mut TrieInputSorted,
    hashed_state: &HashedPostStateSorted,
    trie_updates: &TrieUpdatesSorted,
) {
    #[cfg(feature = "rayon")]
    {
        rayon::join(
            || {
                if !hashed_state.is_empty() {
                    Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
                }
            },
            || {
                if !trie_updates.is_empty() {
                    Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
                }
            },
        );
    }

    #[cfg(not(feature = "rayon"))]
    {
        if !hashed_state.is_empty() {
            Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
        }
        if !trie_updates.is_empty() {
            Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, ComputedTrieData, EthPrimitives, ExecutedBlock};
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage};
    #[cfg(feature = "rayon")]
    use std::time::Instant;
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
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

        let (_, state) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let (_, short) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert_eq!(short.accounts.len(), 1);
        let (_, cached_short) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert!(Arc::ptr_eq(&short, &cached_short));
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

    #[test]
    fn required_lookup_waits_for_in_progress_overlay() {
        let manager = StateTrieOverlayManager::<EthPrimitives>::default();
        let key = OverlayCacheKey {
            anchor_hash: B256::with_last_byte(1),
            tip_hash: B256::with_last_byte(2),
        };
        let waiter = Arc::new(OverlayWaiter::new());
        manager.overlays.insert(key, OverlayCacheEntry::Computing(Arc::clone(&waiter)));

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let res =
                manager.overlay_for_parent(key.tip_hash, key.anchor_hash).map(|(_, state)| state);
            tx.send(res).unwrap();
        });

        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        waiter.finish(Arc::new(TrieInputSorted::default()));

        let state = rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        assert!(state.is_empty());
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn precompute_skips_in_progress_overlay() {
        let manager = StateTrieOverlayManager::<EthPrimitives>::new(Arc::new(WorkerPool::new(
            1,
            "test-ovly",
        )));
        let key = OverlayCacheKey {
            anchor_hash: B256::with_last_byte(1),
            tip_hash: B256::with_last_byte(2),
        };
        manager.overlays.insert(key, OverlayCacheEntry::Computing(Arc::new(OverlayWaiter::new())));

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            tx.send(manager.precompute_overlay(key.tip_hash, key.anchor_hash)).unwrap();
        });

        rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn insert_block_prepares_child_overlay_from_cached_parent() {
        let manager = StateTrieOverlayManager::new(Arc::new(WorkerPool::new(2, "test-ovly")));
        let blocks = test_blocks();

        manager.insert_block(blocks[0].clone());

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[0].recovered_block().hash();
        manager.overlay_for_parent(parent_hash, anchor_hash).unwrap();

        let child_hash = blocks[1].recovered_block().hash();
        manager.insert_block(blocks[1].clone());

        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !manager.overlays.contains_key(&child_key) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for optimistically prepared child overlay"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let (_, state) = manager.overlay_for_parent(child_hash, anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 2);
    }

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let original_anchor = blocks[0].recovered_block().parent_hash();
        manager.overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor).unwrap();

        manager.remove_blocks([
            blocks[0].recovered_block().hash(),
            blocks[1].recovered_block().hash(),
        ]);

        let anchor_hash = blocks[1].recovered_block().hash();
        assert!(manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .is_err());

        let (_, state) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 1);
    }
}
