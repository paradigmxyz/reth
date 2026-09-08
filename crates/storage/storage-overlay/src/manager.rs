//! State trie and execution overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`OverlayManager`] tracks those in-memory blocks and builds
//! reusable state trie and execution overlays on demand.

use crate::{
    changeset_cache::compute_block_trie_updates,
    database_state_frontiers,
    manager_metrics::{ExecutionOverlayMetrics, OverlayCacheMetrics, StateTrieOverlayMetrics},
    ChangesetCache, ExecutionOverlay, OverlayBuilder,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use parking_lot::Mutex;
use reth_chain_state::{BlockState, ExecutedBlock, PreservedSparseTrie};
use reth_errors::ProviderResult;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{
    dashmap::{mapref::entry::Entry, DashMap},
    AlloyBlockHeader, FastInstant, NodePrimitives,
};
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{
    fmt,
    ops::RangeInclusive,
    sync::{Arc, OnceLock},
    time::Instant,
};
use tracing::{debug, trace};

/// Manages state trie and execution overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph, changeset cache, and caches keyed by
/// `(anchor_hash, tip_hash)`.
#[derive(Clone)]
pub struct OverlayManager<N: NodePrimitives = EthPrimitives> {
    blocks: Arc<DashMap<B256, ExecutedBlock<N>>>,
    state_trie_overlays: OverlayCache<TrieInputSorted>,
    execution_overlays: OverlayCache<ExecutionOverlay>,
    changeset_cache: ChangesetCache,
    preserved_sparse_trie: Arc<Mutex<Option<PreservedSparseTrie>>>,
    #[cfg(feature = "rayon")]
    worker_pool: Option<Arc<WorkerPool>>,
    metrics: StateTrieOverlayMetrics,
    execution_metrics: ExecutionOverlayMetrics,
}

impl<N: NodePrimitives> Default for OverlayManager<N> {
    fn default() -> Self {
        Self {
            blocks: Default::default(),
            state_trie_overlays: Default::default(),
            execution_overlays: Default::default(),
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            #[cfg(feature = "rayon")]
            worker_pool: None,
            metrics: Default::default(),
            execution_metrics: Default::default(),
        }
    }
}

impl<N: NodePrimitives> std::fmt::Debug for OverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayManager")
            .field("blocks", &self.blocks.len())
            .field("state_trie_overlays", &self.state_trie_overlays.len())
            .field("execution_overlays", &self.execution_overlays.len())
            .finish()
    }
}

impl<N: NodePrimitives> OverlayManager<N> {
    /// Create a new [`OverlayManager`] backed by the given worker pool.
    #[cfg(feature = "rayon")]
    pub fn new(worker_pool: Arc<WorkerPool>) -> Self {
        Self {
            blocks: Default::default(),
            state_trie_overlays: Default::default(),
            execution_overlays: Default::default(),
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            worker_pool: Some(worker_pool),
            metrics: Default::default(),
            execution_metrics: Default::default(),
        }
    }

    /// Creates an overlay builder for `parent_hash`.
    pub fn overlay_builder(&self, parent_hash: B256) -> OverlayBuilder<N> {
        OverlayBuilder::new(parent_hash, self.block_state(parent_hash), self.clone())
    }

    fn block_state(&self, parent_hash: B256) -> Option<BlockState<N>> {
        let mut blocks = self.parent_chain(parent_hash).collect::<Vec<_>>();
        blocks.pop().map(|oldest| {
            blocks.into_iter().rev().fold(BlockState::new(oldest), |parent, block| {
                BlockState::with_parent(block, Some(Arc::new(parent)))
            })
        })
    }

    pub(crate) const fn changeset_cache(&self) -> &ChangesetCache {
        &self.changeset_cache
    }

    /// Gets or computes cached changesets for an inclusive block range.
    pub fn get_or_compute_cached_changesets_range<P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        let (partial_state_trie, finish) = database_state_frontiers(provider)?;
        self.get_or_compute_cached_changesets_range_at_frontiers(
            provider,
            range,
            partial_state_trie,
            finish,
        )
    }

    pub(crate) fn get_or_compute_cached_changesets_range_at_frontiers<P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
    ) -> ProviderResult<Arc<TrieUpdatesSorted>>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        self.changeset_cache.get_or_compute_range(self, provider, range, partial_state_trie, finish)
    }

    /// Evicts cached changesets for blocks below `up_to_block`.
    pub fn evict_cached_changesets(&self, up_to_block: BlockNumber) {
        self.changeset_cache.evict(up_to_block);
    }

    /// Computes the trie updates produced by `block_number`.
    pub fn compute_block_trie_updates<P>(
        &self,
        provider: &P,
        block_number: BlockNumber,
    ) -> ProviderResult<TrieUpdatesSorted>
    where
        P: DBProvider
            + ChangeSetReader
            + StorageChangeSetReader
            + PruneCheckpointReader
            + StageCheckpointReader
            + BlockNumReader
            + StorageSettingsCache,
    {
        compute_block_trie_updates(self, provider, block_number)
    }

    /// Takes the preserved sparse trie if present.
    pub fn take_sparse_trie(&self) -> Option<PreservedSparseTrie> {
        self.preserved_sparse_trie.lock().take()
    }

    /// Stores a preserved sparse trie for later reuse.
    pub fn store_sparse_trie(&self, trie: PreservedSparseTrie) {
        *self.preserved_sparse_trie.lock() = Some(trie);
    }

    /// Clears any preserved sparse trie state.
    pub fn clear_sparse_trie(&self) {
        *self.preserved_sparse_trie.lock() = None;
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
                target: "storage::overlay::manager",
                blocked_for=?elapsed,
                "Waited for preserved sparse trie to become available"
            );
        }
        elapsed
    }

    /// Inserts an executed in-memory block into the state trie overlay manager.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
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
                    target: "storage::overlay::manager",
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
            .execution_overlays
            .entries
            .iter()
            .filter_map(|entry| {
                let key = *entry.key();
                (key.tip_hash == parent_hash).then_some(key.anchor_hash)
            })
            .collect::<Vec<_>>();

        debug!(
            target: "storage::overlay::manager",
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

        // When a new block is inserted we optimistically and asynchronously flatten an execution
        // overlay for it
        #[cfg(feature = "rayon")]
        {
            let parent_span = span;
            for anchor_hash in cached_parent_overlays {
                let manager = self.clone();
                let parent_span = parent_span.clone();
                worker_pool.spawn(move || {
                    let _span = tracing::trace_span!(
                        target: "storage::overlay::manager",
                        parent: parent_span,
                        "precompute_execution_overlay",
                        tip_hash = %hash,
                        anchor_hash = %anchor_hash,
                    )
                    .entered();
                    let _ = manager.precompute_execution_overlay_for_parent(hash, anchor_hash);
                });
            }
        }
    }

    /// Removes blocks from the live block graph and prunes cached overlays that can no longer be
    /// built from the remaining blocks.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
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
            let overlays_before = self.state_trie_overlays.len() + self.execution_overlays.len();
            self.state_trie_overlays.retain(|key, _| {
                self.contains_hash(key.tip_hash, key.anchor_hash, key.anchor_hash)
            });
            self.execution_overlays.retain(|key, _| {
                self.contains_hash(key.tip_hash, key.anchor_hash, key.anchor_hash)
            });
            pruned_overlays = overlays_before
                .saturating_sub(self.state_trie_overlays.len() + self.execution_overlays.len());
            span.record("pruned_overlays", pruned_overlays);
        }
        debug!(
            target: "storage::overlay::manager",
            block_count,
            removed_blocks,
            pruned_overlays,
            "removed blocks from state trie overlay manager"
        );
    }

    /// Returns the flattened overlay from `anchor_hash` to `parent_hash`.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(tip_hash = %parent_state.hash(), anchor_hash = %anchor_hash)
    )]
    pub(crate) fn overlay_for_parent(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        let parent_hash = parent_state.hash();
        if parent_hash == anchor_hash {
            return Ok((
                Arc::new(TrieUpdatesSorted::default()),
                Arc::new(HashedPostStateSorted::default()),
            ))
        }
        debug!(
            target: "storage::overlay::manager",
            tip_hash = %parent_hash,
            %anchor_hash,
            "loading state trie overlay for parent"
        );
        let input = self
            .get_or_compute_overlay(
                &self.state_trie_overlays,
                &self.metrics,
                anchor_hash,
                parent_state,
                true,
                |input, span| self.compute_state_trie_overlay(input, anchor_hash, span),
            )?
            .expect("required overlay lookup cannot skip an in-progress computation");
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    /// Returns execution data for the in-memory chain from `anchor_hash` to `parent_hash`.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(tip_hash = %parent_state.hash(), anchor_hash = %anchor_hash)
    )]
    pub(crate) fn execution_overlay_for_block_state(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
    ) -> Result<Arc<ExecutionOverlay>, StateTrieOverlayError> {
        Ok(self
            .execution_overlay_for_parent_inner(parent_state, anchor_hash, true)?
            .expect("required overlay lookup cannot skip an in-progress computation"))
    }

    #[cfg(feature = "rayon")]
    fn precompute_execution_overlay_for_parent(
        &self,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(), StateTrieOverlayError> {
        let parent_state = self
            .block_state(parent_hash)
            .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash })?;
        self.execution_overlay_for_parent_inner(&parent_state, anchor_hash, false).map(drop)
    }

    fn execution_overlay_for_parent_inner(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        wait_for_pending: bool,
    ) -> Result<Option<Arc<ExecutionOverlay>>, StateTrieOverlayError> {
        let parent_hash = parent_state.hash();
        if parent_hash == anchor_hash {
            return Ok(Some(Arc::new(ExecutionOverlay::default())))
        }

        self.get_or_compute_overlay(
            &self.execution_overlays,
            &self.execution_metrics,
            anchor_hash,
            parent_state,
            wait_for_pending,
            |input, span| self.compute_execution_overlay(input, anchor_hash, span),
        )
    }

    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(
            tip_hash = %parent_state.hash(),
            anchor_hash = %anchor_hash,
            cache_reused = tracing::field::Empty,
            block_count = tracing::field::Empty,
            parent_overlay_reused = tracing::field::Empty,
        )
    )]
    fn get_or_compute_overlay<T, M>(
        &self,
        cache: &OverlayCache<T>,
        metrics: &M,
        anchor_hash: B256,
        parent_state: &BlockState<N>,
        wait_for_pending: bool,
        compute: impl FnOnce(ComputeOverlayInput<N, T>, tracing::Span) -> T,
    ) -> Result<Option<Arc<T>>, StateTrieOverlayError>
    where
        M: OverlayCacheMetrics,
    {
        let tip_hash = parent_state.hash();
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();
        if let Some(entry) = cache.entries.get(&key).map(|entry| entry.value().clone()) {
            metrics.record_cache_reuse();
            span.record("cache_reused", true);
            return match entry {
                OverlayCacheEntry::Ready(input) => Ok(Some(input)),
                OverlayCacheEntry::Computing(waiter) if wait_for_pending => Ok(Some(waiter.wait())),
                OverlayCacheEntry::Computing(_) => Ok(None),
            }
        }
        span.record("cache_reused", false);

        enum CacheAction<T> {
            Ready(Arc<T>),
            Wait(Arc<OverlayWaiter<T>>),
            Compute(Arc<OverlayWaiter<T>>),
        }

        let action = match cache.entries.entry(key) {
            Entry::Occupied(entry) => {
                let entry = entry.get().clone();
                metrics.record_cache_reuse();
                span.record("cache_reused", true);
                match entry {
                    OverlayCacheEntry::Ready(input) => CacheAction::Ready(input),
                    OverlayCacheEntry::Computing(_) if !wait_for_pending => return Ok(None),
                    OverlayCacheEntry::Computing(waiter) => CacheAction::Wait(waiter),
                }
            }
            Entry::Vacant(entry) => {
                metrics.record_cache_fill();
                let waiter = Arc::new(OverlayWaiter::new());
                entry.insert(OverlayCacheEntry::Computing(Arc::clone(&waiter)));
                CacheAction::Compute(waiter)
            }
        };

        match action {
            CacheAction::Ready(input) => Ok(Some(input)),
            CacheAction::Wait(waiter) => Ok(Some(waiter.wait())),
            CacheAction::Compute(waiter) => {
                let (blocks, parent_input) =
                    Self::blocks_from_parent_state(parent_state, anchor_hash, cache)?;
                span.record("block_count", blocks.len());
                span.record("parent_overlay_reused", parent_input.is_some());
                let compute_input = match parent_input {
                    Some(parent_input) => {
                        ComputeOverlayInput::ExtendCached { blocks, parent_input }
                    }
                    None => ComputeOverlayInput::MergeBlocks(blocks),
                };
                let input = Arc::new(compute(compute_input, span));
                waiter.finish(Arc::clone(&input));

                if let Entry::Occupied(mut entry) = cache.entries.entry(key) {
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

    fn blocks_from_parent_state<T>(
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache: &OverlayCache<T>,
    ) -> Result<BlocksAndParentOverlay<N, T>, StateTrieOverlayError> {
        let tip_hash = parent_state.hash();
        let mut hash = tip_hash;
        let mut blocks = Vec::new();
        for state in parent_state.chain() {
            let block = state.block();
            if block.recovered_block().hash() != hash {
                return Err(StateTrieOverlayError { tip_hash, anchor_hash })
            }
            hash = block.recovered_block().parent_hash();
            blocks.push(block);
            if hash == anchor_hash {
                return Ok((blocks, None))
            }
            if let Some(parent_input) =
                cache.take_ready(&OverlayCacheKey { anchor_hash, tip_hash: hash })
            {
                return Ok((blocks, Some(parent_input)))
            }
        }
        Err(StateTrieOverlayError { tip_hash, anchor_hash })
    }

    /// Returns every in-memory block in the chain whose tip is `parent_hash`.
    fn parent_chain(&self, parent_hash: B256) -> impl Iterator<Item = ExecutedBlock<N>> + '_ {
        let mut hash = parent_hash;
        std::iter::from_fn(move || {
            let block = self.blocks.get(&hash)?;
            hash = block.recovered_block().parent_hash();
            Some(block.clone())
        })
    }

    /// Returns true if `hash` is in the parent chain segment from `anchor_hash` inclusive to
    /// `parent_hash` inclusive.
    fn contains_hash(&self, parent_hash: B256, anchor_hash: B256, hash: B256) -> bool {
        let mut current_hash = parent_hash;

        loop {
            if current_hash == hash {
                return true
            }
            if current_hash == anchor_hash {
                return false
            }

            let Some(block) = self.blocks.get(&current_hash) else { return false };
            current_hash = block.recovered_block().parent_hash();
        }
    }

    fn compute_state_trie_overlay(
        &self,
        compute_input: ComputeOverlayInput<N, TrieInputSorted>,
        anchor_hash: B256,
        _span: tracing::Span,
    ) -> TrieInputSorted {
        #[cfg(feature = "rayon")]
        {
            if let Some(worker_pool) = &self.worker_pool {
                let compute_span = _span;
                let metrics = self.metrics.clone();
                return worker_pool.spawn_and_wait(move || {
                    let _guard = compute_span.enter();
                    compute_overlay(compute_input, anchor_hash, &metrics)
                })
            }
        }

        compute_overlay(compute_input, anchor_hash, &self.metrics)
    }

    fn compute_execution_overlay(
        &self,
        compute_input: ComputeOverlayInput<N, ExecutionOverlay>,
        anchor_hash: B256,
        _span: tracing::Span,
    ) -> ExecutionOverlay {
        #[cfg(feature = "rayon")]
        {
            if let Some(worker_pool) = &self.worker_pool {
                let compute_span = _span;
                let metrics = self.execution_metrics.clone();
                return worker_pool.spawn_and_wait(move || {
                    let _guard = compute_span.enter();
                    compute_execution_overlay_inner(compute_input, anchor_hash, &metrics)
                })
            }
        }

        compute_execution_overlay_inner(compute_input, anchor_hash, &self.execution_metrics)
    }
}

/// Error returned when a state trie overlay cannot be built from the manager's current block set.
#[derive(Debug)]
pub(crate) struct StateTrieOverlayError {
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

struct OverlayCache<T> {
    entries: Arc<DashMap<OverlayCacheKey, OverlayCacheEntry<T>>>,
}

impl<T> Default for OverlayCache<T> {
    fn default() -> Self {
        Self { entries: Default::default() }
    }
}

impl<T> Clone for OverlayCache<T> {
    fn clone(&self) -> Self {
        Self { entries: Arc::clone(&self.entries) }
    }
}

impl<T> OverlayCache<T> {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn retain(&self, mut keep: impl FnMut(&OverlayCacheKey, &OverlayCacheEntry<T>) -> bool) {
        self.entries.retain(|key, entry| keep(key, entry))
    }

    /// Removes and returns a ready entry.
    ///
    /// Transferring a parent entry lets `Arc::make_mut` extend it in place when no caller retains
    /// it. Keeping the cache entry would otherwise guarantee a clone.
    fn take_ready(&self, key: &OverlayCacheKey) -> Option<Arc<T>> {
        let (_, entry) =
            self.entries.remove_if(key, |_, entry| matches!(entry, OverlayCacheEntry::Ready(_)))?;
        let OverlayCacheEntry::Ready(input) = entry else { unreachable!() };
        Some(input)
    }
}

enum OverlayCacheEntry<T> {
    Ready(Arc<T>),
    Computing(Arc<OverlayWaiter<T>>),
}

impl<T> Clone for OverlayCacheEntry<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Ready(input) => Self::Ready(Arc::clone(input)),
            Self::Computing(waiter) => Self::Computing(Arc::clone(waiter)),
        }
    }
}

struct OverlayWaiter<T> {
    input: OnceLock<Arc<T>>,
}

impl<T> OverlayWaiter<T> {
    const fn new() -> Self {
        Self { input: OnceLock::new() }
    }

    fn wait(&self) -> Arc<T> {
        Arc::clone(self.input.wait())
    }

    fn finish(&self, computed: Arc<T>) {
        let _ = self.input.set(computed);
    }
}

enum ComputeOverlayInput<N: NodePrimitives, T> {
    ExtendCached { blocks: Vec<ExecutedBlock<N>>, parent_input: Arc<T> },
    MergeBlocks(Vec<ExecutedBlock<N>>),
}

type BlocksAndParentOverlay<N, T> = (Vec<ExecutedBlock<N>>, Option<Arc<T>>);

#[tracing::instrument(
    level = "trace",
    target = "storage::overlay::manager",
    skip_all,
    fields(
        anchor_hash = %anchor_hash,
        block_count = tracing::field::Empty,
        parent_overlay = tracing::field::Empty,
        elapsed_us = tracing::field::Empty,
    )
)]
fn compute_overlay<N: NodePrimitives>(
    input: ComputeOverlayInput<N, TrieInputSorted>,
    anchor_hash: B256,
    metrics: &StateTrieOverlayMetrics,
) -> TrieInputSorted {
    let started_at = Instant::now();
    let block_count = match &input {
        ComputeOverlayInput::ExtendCached { blocks, .. } |
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { blocks, parent_input } => {
            let head = blocks
                .first()
                .expect("cached parent requires a child block")
                .recovered_block()
                .hash();
            trace!(
                target: "storage::overlay::manager",
                %anchor_hash,
                %head,
                "extending cached parent state trie overlay"
            );

            let mut parent_input = parent_input;
            let extension = merge_blocks(blocks);
            extend_overlay(Arc::make_mut(&mut parent_input), &extension.state, &extension.nodes);
            Arc::try_unwrap(parent_input).expect("Arc::make_mut leaves the child overlay unique")
        }
        ComputeOverlayInput::MergeBlocks(blocks) => merge_blocks(blocks),
    };

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "storage::overlay::manager",
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
                trie_data.iter().map(|data| Arc::clone(&data.sorted.trie_updates)),
            )
        },
        || {
            HashedPostStateSorted::merge_batch(
                trie_data.iter().map(|data| Arc::clone(&data.sorted.hashed_state)),
            )
        },
    );

    #[cfg(not(feature = "rayon"))]
    let (nodes, state) = (
        TrieUpdatesSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.sorted.trie_updates)),
        ),
        HashedPostStateSorted::merge_batch(
            trie_data.iter().map(|data| Arc::clone(&data.sorted.hashed_state)),
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

fn compute_execution_overlay_inner<N: NodePrimitives>(
    input: ComputeOverlayInput<N, ExecutionOverlay>,
    anchor_hash: B256,
    metrics: &ExecutionOverlayMetrics,
) -> ExecutionOverlay {
    let started_at = Instant::now();
    let block_count = match &input {
        ComputeOverlayInput::ExtendCached { blocks, .. } |
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { blocks, parent_input } => {
            let mut parent_input = parent_input;
            let overlay = Arc::make_mut(&mut parent_input);
            for block in blocks.iter().rev() {
                overlay.extend_block(block);
            }
            Arc::try_unwrap(parent_input).expect("Arc::make_mut leaves the child overlay unique")
        }
        ComputeOverlayInput::MergeBlocks(blocks) => {
            let mut overlay = ExecutionOverlay::default();
            for block in blocks.iter().rev() {
                overlay.extend_block(block);
            }
            overlay
        }
    };

    let elapsed = started_at.elapsed();
    metrics.overlay_computation_duration_seconds.record(elapsed.as_secs_f64());
    tracing::Span::current().record("elapsed_us", elapsed.as_micros() as u64);
    debug!(
        target: "storage::overlay::manager",
        %anchor_hash,
        block_count,
        parent_overlay,
        ?elapsed,
        "computed execution overlay"
    );

    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::HashMap, Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock, SparseTrie};
    use reth_ethereum_primitives::EthPrimitives;
    use reth_primitives_traits::Account;
    #[cfg(feature = "rayon")]
    use reth_tasks::WorkerPool;
    use reth_trie::{updates::TrieUpdatesSorted, ComputedTrieData, HashedPostState, HashedStorage};
    use revm::{
        bytecode::Bytecode,
        database::BundleState,
        state::{AccountId, AccountInfo},
    };
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
                HashedStorage::from_iter([(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();
        let address = Address::with_last_byte(id);
        let slot = U256::from(id);
        let code_hash = B256::with_last_byte(id.saturating_add(64));
        let state = BundleState::builder(block.block_number()..=block.block_number())
            .state_present_account_info(
                address,
                AccountInfo {
                    nonce: id as u64,
                    balance: U256::from(id),
                    account_id: AccountId::new(id as usize),
                    ..Default::default()
                },
            )
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(id)))]))
            .contract(code_hash, Bytecode::new_raw(vec![id].into()))
            .build();
        let mut execution_output = (*block.execution_output).clone();
        execution_output.state = state;

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::new(execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(TrieUpdatesSorted::default())),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        test_blocks_with_count(3)
    }

    fn test_blocks_with_count(count: u64) -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..count + 1)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    impl OverlayManager {
        fn execution_overlay_for_parent(
            &self,
            parent_hash: B256,
            anchor_hash: B256,
        ) -> Result<Arc<ExecutionOverlay>, StateTrieOverlayError> {
            if parent_hash == anchor_hash {
                return Ok(Arc::new(ExecutionOverlay::default()))
            }
            let parent_state = self
                .block_state(parent_hash)
                .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash })?;
            self.execution_overlay_for_block_state(&parent_state, anchor_hash)
        }
    }

    fn overlay_for_parent(
        manager: &OverlayManager,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        let parent_state = manager
            .block_state(parent_hash)
            .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash })?;
        manager.overlay_for_parent(&parent_state, anchor_hash)
    }

    #[test]
    fn errors_for_unknown_parent() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let parent = B256::random();
        let anchor = B256::random();

        let err = overlay_for_parent(&manager, parent, anchor).unwrap_err();

        assert_eq!(err.tip_hash, parent);
        assert_eq!(err.anchor_hash, anchor);
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();

        let (_, state) =
            overlay_for_parent(&manager, blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let (_, short) =
            overlay_for_parent(&manager, blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert_eq!(short.accounts.len(), 1);
        let (_, cached_short) =
            overlay_for_parent(&manager, blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert!(Arc::ptr_eq(&short, &cached_short));
    }

    #[test]
    fn builds_execution_overlay_for_inserted_blocks() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let overlay = manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap();

        for id in 1..=3 {
            let address = Address::with_last_byte(id);
            let code_hash = B256::with_last_byte(id + 64);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().nonce, id as u64);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().account_id, None);
            assert_eq!(overlay.storage()[&address][&U256::from(id)], U256::from(id));
            assert_eq!(overlay.code_hashes()[&code_hash], Bytecode::new_raw(vec![id].into()));
        }
        assert_eq!(
            overlay.block_hashes(),
            blocks[..=2].iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(),
        );

        let cached = manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap();
        assert!(Arc::ptr_eq(&overlay, &cached));

        let short_anchor = blocks[1].recovered_block().hash();
        let short = manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor)
            .unwrap();
        assert_eq!(short.accounts().len(), 1);
    }

    #[test]
    fn execution_overlay_for_parent_at_anchor_is_empty() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let anchor_hash = B256::with_last_byte(1);

        let overlay = manager.execution_overlay_for_parent(anchor_hash, anchor_hash).unwrap();

        assert!(overlay.accounts().is_empty());
        assert!(overlay.storage().is_empty());
        assert!(overlay.code_hashes().is_empty());
        assert!(overlay.block_hashes().is_empty());
    }

    #[test]
    fn promotes_ready_parent_overlays_to_the_child() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };

        overlay_for_parent(&manager, parent_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(parent_hash, anchor_hash).unwrap();

        overlay_for_parent(&manager, child_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(child_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(manager.state_trie_overlays.entries.contains_key(&child_key));
        assert!(!manager.execution_overlays.entries.contains_key(&parent_key));
        assert!(manager.execution_overlays.entries.contains_key(&child_key));
    }

    #[test]
    fn promotes_parent_overlays_held_by_callers() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };

        overlay_for_parent(&manager, parent_hash, anchor_hash).unwrap();
        let state_parent = manager
            .state_trie_overlays
            .entries
            .get(&parent_key)
            .and_then(|entry| match entry.value() {
                OverlayCacheEntry::Ready(input) => Some(Arc::clone(input)),
                OverlayCacheEntry::Computing(_) => None,
            })
            .unwrap();
        let execution_parent =
            manager.execution_overlay_for_parent(parent_hash, anchor_hash).unwrap();

        let (_, child_state) = overlay_for_parent(&manager, child_hash, anchor_hash).unwrap();
        let child_execution =
            manager.execution_overlay_for_parent(child_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(!manager.execution_overlays.entries.contains_key(&parent_key));
        assert_eq!(state_parent.state.accounts.len(), 2);
        assert_eq!(execution_parent.accounts().len(), 2);
        assert_eq!(child_state.accounts.len(), 3);
        assert_eq!(child_execution.accounts().len(), 3);
        assert!(child_execution
            .accounts()
            .values()
            .flatten()
            .all(|account| account.account_id.is_none()));
    }

    #[test]
    fn reuses_nearest_ready_ancestor_overlay() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let ancestor_hash = blocks[0].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let ancestor_key = OverlayCacheKey { anchor_hash, tip_hash: ancestor_hash };

        overlay_for_parent(&manager, ancestor_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(ancestor_hash, anchor_hash).unwrap();

        let (_, child_state) = overlay_for_parent(&manager, child_hash, anchor_hash).unwrap();
        let child_execution =
            manager.execution_overlay_for_parent(child_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&ancestor_key));
        assert!(!manager.execution_overlays.entries.contains_key(&ancestor_key));
        assert_eq!(child_state.accounts.len(), 3);
        assert_eq!(child_execution.accounts().len(), 3);
    }

    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_parent_overlay_reuse() {
        const DEPTH: u64 = 128;
        const ITERATIONS: usize = 20;

        let manager = OverlayManager::default();
        let blocks = test_blocks_with_count(DEPTH);
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let tip_hash = blocks.last().unwrap().recovered_block().hash();
        let parent_hash = blocks[blocks.len() - 2].recovered_block().hash();

        let mut uncached = Duration::ZERO;
        for _ in 0..ITERATIONS {
            manager.state_trie_overlays.entries.clear();
            let start = Instant::now();
            std::hint::black_box(overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap());
            uncached += start.elapsed();
        }

        let mut parent_ready = Duration::ZERO;
        for _ in 0..ITERATIONS {
            manager.state_trie_overlays.entries.clear();
            overlay_for_parent(&manager, parent_hash, anchor_hash).unwrap();
            let start = Instant::now();
            std::hint::black_box(overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap());
            parent_ready += start.elapsed();
        }

        eprintln!(
            "depth={DEPTH}, iterations={ITERATIONS}, uncached_avg={:?}, parent_ready_avg={:?}",
            uncached / ITERATIONS as u32,
            parent_ready / ITERATIONS as u32,
        );
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn precomputes_execution_overlay_for_cached_parent() {
        let manager = OverlayManager::new(Arc::new(WorkerPool::new(1, "execution-overlay-test")));
        let blocks = test_blocks();
        let anchor_hash = blocks[0].recovered_block().parent_hash();

        manager.insert_block(blocks[0].clone());
        manager
            .execution_overlay_for_parent(blocks[0].recovered_block().hash(), anchor_hash)
            .unwrap();

        manager.insert_block(blocks[1].clone());
        let key = OverlayCacheKey { anchor_hash, tip_hash: blocks[1].recovered_block().hash() };
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !manager
            .execution_overlays
            .entries
            .get(&key)
            .is_some_and(|entry| matches!(entry.value(), OverlayCacheEntry::Ready(_)))
        {
            assert!(std::time::Instant::now() < deadline, "execution overlay was not precomputed");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!manager.execution_overlays.entries.contains_key(&OverlayCacheKey {
            anchor_hash,
            tip_hash: blocks[0].recovered_block().hash(),
        }));
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn execution_overlay_precompute_does_not_wait_for_pending_entry() {
        let worker_pool = Arc::new(WorkerPool::new(1, "execution-overlay-pending-test"));
        let manager = OverlayManager::new(Arc::clone(&worker_pool));
        let block = test_blocks().remove(0);
        let anchor_hash = block.recovered_block().parent_hash();
        let tip_hash = block.recovered_block().hash();
        manager.insert_block(block);

        let waiter = Arc::new(OverlayWaiter::new());
        manager.execution_overlays.entries.insert(
            OverlayCacheKey { anchor_hash, tip_hash },
            OverlayCacheEntry::Computing(Arc::clone(&waiter)),
        );

        let (tx, rx) = mpsc::channel();
        worker_pool.spawn(move || {
            manager.precompute_execution_overlay_for_parent(tip_hash, anchor_hash).unwrap();
            tx.send(()).unwrap();
        });

        let completed = rx.recv_timeout(Duration::from_millis(100));
        waiter.finish(Arc::new(ExecutionOverlay::default()));
        assert!(completed.is_ok(), "execution overlay precompute waited for pending entry");
    }

    #[test]
    fn contains_hash_detects_hashes_from_anchor_to_parent() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[2].recovered_block().hash();

        assert!(manager.contains_hash(parent_hash, anchor_hash, anchor_hash));
        for block in &blocks {
            assert!(manager.contains_hash(
                parent_hash,
                anchor_hash,
                block.recovered_block().hash()
            ));
        }
        assert!(!manager.contains_hash(parent_hash, anchor_hash, B256::random()));
    }

    #[test]
    fn contains_hash_rejects_hash_before_anchor() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let parent_hash = blocks[2].recovered_block().hash();
        let anchor_hash = blocks[1].recovered_block().hash();
        let before_anchor_hash = blocks[0].recovered_block().hash();

        assert!(manager.contains_hash(parent_hash, anchor_hash, parent_hash));
        assert!(manager.contains_hash(parent_hash, anchor_hash, anchor_hash));
        assert!(!manager.contains_hash(parent_hash, anchor_hash, before_anchor_hash));
    }

    #[test]
    fn contains_hash_rejects_unknown_anchor() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let parent_hash = blocks[2].recovered_block().hash();
        let anchor_hash = B256::random();

        assert!(!manager.contains_hash(parent_hash, anchor_hash, anchor_hash));
    }

    #[test]
    fn taking_sparse_trie_removes_it() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let state_root = B256::with_last_byte(1);
        let other_state_root = B256::with_last_byte(2);
        let anchor_hash = B256::with_last_byte(3);

        manager.store_sparse_trie(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            state_root,
            anchor_hash,
        ));

        let preserved = manager.take_sparse_trie().expect("preserved trie should be available");
        assert_eq!(preserved.state_root(), state_root);
        assert_eq!(preserved.anchor_hash(), anchor_hash);
        assert!(preserved.into_trie_for(other_state_root).unwrap().is_none());
        assert!(manager.take_sparse_trie().is_none());
    }

    #[test]
    fn required_lookup_waits_for_in_progress_overlay() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let block = test_blocks().remove(0);
        let parent_state = BlockState::new(block);
        let key = OverlayCacheKey {
            anchor_hash: parent_state.block_ref().recovered_block().parent_hash(),
            tip_hash: parent_state.hash(),
        };
        let waiter = Arc::new(OverlayWaiter::new());
        manager
            .state_trie_overlays
            .entries
            .insert(key, OverlayCacheEntry::Computing(Arc::clone(&waiter)));

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let res =
                manager.overlay_for_parent(&parent_state, key.anchor_hash).map(|(_, state)| state);
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

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = OverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let original_anchor = blocks[0].recovered_block().parent_hash();
        overlay_for_parent(&manager, blocks[2].recovered_block().hash(), original_anchor).unwrap();
        manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .unwrap();

        manager.remove_blocks([
            blocks[0].recovered_block().hash(),
            blocks[1].recovered_block().hash(),
        ]);

        let anchor_hash = blocks[1].recovered_block().hash();
        assert!(overlay_for_parent(&manager, blocks[2].recovered_block().hash(), original_anchor)
            .is_err());
        assert!(manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .is_err());

        let (_, state) =
            overlay_for_parent(&manager, blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 1);
        let execution = manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash)
            .unwrap();
        assert_eq!(execution.accounts().len(), 1);
    }
}
