//! State trie and execution overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`OverlayManager`] builds reusable state trie and execution
//! overlays for those blocks on demand, from the shared [`BlockState`] chains their callers
//! already hold. It neither owns nor looks up an in-memory block itself.

use crate::{
    changeset_cache::compute_block_trie_updates,
    database_state_frontiers,
    manager_metrics::{ExecutionOverlayMetrics, OverlayCacheMetrics, StateTrieOverlayMetrics},
    ChangesetCache, ExecutionOverlay, OverlayBuilder,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Set, BlockNumber, B256};
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
    marker::PhantomData,
    ops::RangeInclusive,
    sync::{Arc, OnceLock},
    time::Instant,
};
use tracing::{debug, trace};

/// Manages state trie and execution overlays for in-memory blocks.
///
/// The manager neither owns nor looks up in-memory blocks: every input is a [`BlockState`] chain
/// that the caller already holds, or a block number. What it owns is the changeset cache, the
/// preserved sparse trie, and the flattened overlays keyed by `(anchor_hash, tip_hash)`.
#[derive(Clone)]
pub struct OverlayManager<N: NodePrimitives = EthPrimitives> {
    /// The manager is generic over the primitives its callers' chains use, but stores no block.
    _primitives: PhantomData<fn() -> N>,
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
            _primitives: PhantomData,
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
            _primitives: PhantomData,
            state_trie_overlays: Default::default(),
            execution_overlays: Default::default(),
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            worker_pool: Some(worker_pool),
            metrics: Default::default(),
            execution_metrics: Default::default(),
        }
    }

    /// Creates an overlay builder for an already materialized in-memory chain.
    ///
    /// The chain tip is used as the parent hash, so no lookup and no chain rebuild is performed.
    /// The builder only reads `state`, which makes it safe to share the same [`BlockState`] with
    /// its owner and with every other holder.
    pub fn overlay_builder_for_state(&self, state: Arc<BlockState<N>>) -> OverlayBuilder<N> {
        OverlayBuilder::new(state.hash(), Some(state), self.clone())
    }

    /// Creates an overlay builder for a block that is already durable in the database.
    ///
    /// No in-memory chain is attached, because there is none above a persisted block. Callers
    /// that have a chain use [`Self::overlay_builder_for_state`] instead.
    pub fn overlay_builder_for_persisted(&self, parent_hash: B256) -> OverlayBuilder<N> {
        OverlayBuilder::new(parent_hash, None, self.clone())
    }

    pub(crate) const fn changeset_cache(&self) -> &ChangesetCache {
        &self.changeset_cache
    }

    /// Gets or computes cached changesets for an inclusive block range.
    ///
    /// `finish_state` is the in-memory chain ending at the Finish frontier. It is only consulted
    /// when the cache misses and the aggregate fallback has to complete a masked state trie;
    /// callers with nothing in memory above the database tip pass `None`.
    pub fn get_or_compute_cached_changesets_range<P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
        finish_state: Option<Arc<BlockState<N>>>,
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
            finish_state,
        )
    }

    pub(crate) fn get_or_compute_cached_changesets_range_at_frontiers<P>(
        &self,
        provider: &P,
        range: RangeInclusive<BlockNumber>,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
        finish_state: Option<Arc<BlockState<N>>>,
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
        self.changeset_cache.get_or_compute_range(
            self,
            provider,
            range,
            partial_state_trie,
            finish,
            finish_state,
        )
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
        finish_state: Option<Arc<BlockState<N>>>,
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
        compute_block_trie_updates(self, provider, block_number, finish_state)
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

    /// Notifies the manager that a new in-memory block was executed on top of `state`'s parent.
    ///
    /// The manager does not store the block. It uses the shared chain to optimistically extend
    /// the overlays it already has cached for the parent.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(
            block_hash = %state.hash(),
            parent_hash = %state.block_ref().recovered_block().parent_hash(),
        )
    )]
    pub fn insert_block(&self, state: Arc<BlockState<N>>) {
        let hash = state.hash();
        let parent_hash = state.block_ref().recovered_block().parent_hash();

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

        #[cfg(not(feature = "rayon"))]
        let _ = (cached_parent_overlays, state);

        // When a new block is inserted we optimistically and asynchronously flatten an execution
        // overlay for it
        #[cfg(feature = "rayon")]
        {
            for anchor_hash in cached_parent_overlays {
                self.precompute_execution_overlay(Arc::clone(&state), anchor_hash);
            }
        }
    }

    /// Optimistically computes an execution overlay from `anchor_hash` to `tip_hash`.
    ///
    /// This returns without waiting for computation. No work is scheduled without a worker pool
    /// or when the tip is already persisted at the anchor. A concurrent persistence or reorg may
    /// make the requested range unavailable, in which case the background task skips it.
    #[cfg(feature = "rayon")]
    pub fn precompute_execution_overlay(&self, tip: Arc<BlockState<N>>, anchor_hash: B256) {
        let tip_hash = tip.hash();
        if tip_hash == anchor_hash {
            return
        }
        let Some(worker_pool) = &self.worker_pool else { return };
        let manager = self.clone();
        let parent_span = tracing::Span::current();
        worker_pool.spawn(move || {
            let _span = tracing::trace_span!(
                target: "storage::overlay::manager",
                parent: parent_span,
                "precompute_execution_overlay",
                %tip_hash,
                %anchor_hash,
            )
            .entered();
            if let Err(err) = manager.execution_overlay_for_parent_inner(
                &tip,
                anchor_hash,
                OverlayCacheConfig { precompute: true, write_to_cache: true },
            ) {
                debug!(target: "storage::overlay::manager", %err, "Skipping execution overlay precompute");
            }
        });
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
    pub fn remove_blocks(
        &self,
        hashes: impl IntoIterator<Item = B256>,
        state_trie_frontier: BlockNumber,
    ) {
        let span = tracing::Span::current();

        let removed = hashes.into_iter().collect::<B256Set>();
        let block_count = removed.len();
        span.record("block_count", block_count);
        span.record("removed_blocks", block_count);

        let overlays_before = self.state_trie_overlays.len() + self.execution_overlays.len();
        self.state_trie_overlays
            .retain(|key, cached| overlay_is_live(key, cached, &removed, state_trie_frontier));
        self.execution_overlays.retain(|key, cached| {
            // A ready execution overlay records the blocks it covers, so an interior removal is
            // caught exactly rather than inferred from the ends of the range.
            if let OverlayCacheEntry::Ready(overlay) = &cached.entry &&
                overlay.block_hashes().iter().any(|block| removed.contains(&block.hash))
            {
                return false
            }
            overlay_is_live(key, cached, &removed, state_trie_frontier)
        });
        let pruned_overlays = overlays_before
            .saturating_sub(self.state_trie_overlays.len() + self.execution_overlays.len());
        span.record("pruned_overlays", pruned_overlays);

        debug!(
            target: "storage::overlay::manager",
            block_count,
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
        cache_config: OverlayCacheConfig,
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
                cache_config,
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
        cache_config: OverlayCacheConfig,
    ) -> Result<Arc<ExecutionOverlay>, StateTrieOverlayError> {
        Ok(self
            .execution_overlay_for_parent_inner(parent_state, anchor_hash, cache_config)?
            .expect("required overlay lookup cannot skip an in-progress computation"))
    }

    fn execution_overlay_for_parent_inner(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
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
            cache_config,
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
        cache_config: OverlayCacheConfig,
        compute: impl FnOnce(ComputeOverlayInput<N, T>, tracing::Span) -> T,
    ) -> Result<Option<Arc<T>>, StateTrieOverlayError>
    where
        M: OverlayCacheMetrics,
    {
        let tip_hash = parent_state.hash();
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();
        if let Some(entry) = cache.entries.get(&key).map(|cached| cached.value().entry.clone()) {
            metrics.record_cache_reuse();
            span.record("cache_reused", true);
            return match entry {
                OverlayCacheEntry::Ready(input) => Ok(Some(input)),
                OverlayCacheEntry::Computing(_) if cache_config.precompute => Ok(None),
                OverlayCacheEntry::Computing(waiter) => Ok(Some(waiter.wait())),
            }
        }
        span.record("cache_reused", false);

        // Resolve the block path and any cached parent overlay before locking the child entry.
        let mut blocks = Self::blocks_from_parent_state(parent_state, anchor_hash)?;
        span.record("block_count", blocks.len());

        // `blocks` runs from the tip down to the block right above the anchor, so the anchor's
        // number is known here, before an entry is installed.
        let anchor_number = blocks
            .last()
            .expect("a non-empty block path always reaches the anchor")
            .recovered_block()
            .number()
            .saturating_sub(1);

        if !cache_config.write_to_cache {
            let parent_input = blocks.first().and_then(|block| {
                let parent_hash = block.recovered_block().parent_hash();
                (parent_hash != anchor_hash)
                    .then(|| cache.ready(&OverlayCacheKey { anchor_hash, tip_hash: parent_hash }))
                    .flatten()
            });
            span.record("parent_overlay_reused", parent_input.is_some());
            let compute_input = match parent_input {
                Some(parent_input) => {
                    ComputeOverlayInput::ExtendCached { block: blocks.swap_remove(0), parent_input }
                }
                None => ComputeOverlayInput::MergeBlocks(blocks),
            };
            return Ok(Some(Arc::new(compute(compute_input, span))))
        }

        enum CacheAction<T> {
            Ready(Arc<T>),
            Wait(Arc<OverlayWaiter<T>>),
            Compute(Arc<OverlayWaiter<T>>),
        }

        let action = match cache.entries.entry(key) {
            Entry::Occupied(entry) => {
                let entry = entry.get().entry.clone();
                metrics.record_cache_reuse();
                span.record("cache_reused", true);
                match entry {
                    OverlayCacheEntry::Ready(input) => CacheAction::Ready(input),
                    OverlayCacheEntry::Computing(_) if cache_config.precompute => return Ok(None),
                    OverlayCacheEntry::Computing(waiter) => CacheAction::Wait(waiter),
                }
            }
            Entry::Vacant(entry) => {
                metrics.record_cache_fill();
                let waiter = Arc::new(OverlayWaiter::new());
                entry.insert(CachedOverlay {
                    anchor_number,
                    entry: OverlayCacheEntry::Computing(Arc::clone(&waiter)),
                });
                CacheAction::Compute(waiter)
            }
        };

        match action {
            CacheAction::Ready(input) => Ok(Some(input)),
            CacheAction::Wait(waiter) => Ok(Some(waiter.wait())),
            CacheAction::Compute(waiter) => {
                let parent_input = blocks.first().and_then(|block| {
                    let parent_hash = block.recovered_block().parent_hash();
                    (parent_hash != anchor_hash)
                        .then(|| {
                            cache
                                .take_ready(&OverlayCacheKey { anchor_hash, tip_hash: parent_hash })
                        })
                        .flatten()
                });
                span.record("parent_overlay_reused", parent_input.is_some());
                let compute_input = match parent_input {
                    Some(parent_input) => ComputeOverlayInput::ExtendCached {
                        block: blocks.swap_remove(0),
                        parent_input,
                    },
                    None => ComputeOverlayInput::MergeBlocks(blocks),
                };
                let input = Arc::new(compute(compute_input, span));
                waiter.finish(Arc::clone(&input));

                if let Entry::Occupied(mut entry) = cache.entries.entry(key) {
                    // The entry may have been pruned while the overlay was computing. Only cache
                    // the result if the map still points at the waiter installed by this task.
                    let should_publish = match &entry.get().entry {
                        OverlayCacheEntry::Computing(existing) => Arc::ptr_eq(existing, &waiter),
                        OverlayCacheEntry::Ready(_) => false,
                    };
                    if should_publish {
                        entry.insert(CachedOverlay {
                            anchor_number,
                            entry: OverlayCacheEntry::Ready(Arc::clone(&input)),
                        });
                    }
                }

                Ok(Some(input))
            }
        }
    }

    fn blocks_from_parent_state(
        parent_state: &BlockState<N>,
        anchor_hash: B256,
    ) -> Result<Vec<ExecutedBlock<N>>, StateTrieOverlayError> {
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
                return Ok(blocks)
            }
        }
        Err(StateTrieOverlayError { tip_hash, anchor_hash })
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

/// Returns whether a cached overlay can still be requested.
///
/// An overlay spans the blocks in `(anchor, tip]`. It is dead once either end is gone, or once
/// the durable state trie frontier has moved past its anchor: an overlay anchored below the
/// frontier can never be asked for again, because anchors are resolved at the frontier. The
/// number check also covers an anchor that was never an in-memory block, which no hash-based
/// check can see.
fn overlay_is_live<T>(
    key: &OverlayCacheKey,
    cached: &CachedOverlay<T>,
    removed: &B256Set,
    state_trie_frontier: BlockNumber,
) -> bool {
    if removed.contains(&key.tip_hash) || removed.contains(&key.anchor_hash) {
        return false
    }
    cached.anchor_number >= state_trie_frontier
}

/// Controls how an overlay computation interacts with the manager cache.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OverlayCacheConfig {
    /// Whether this is a best-effort cache fill that must not wait for an existing computation.
    pub(crate) precompute: bool,
    /// Whether to retain the computed overlay in the manager cache.
    pub(crate) write_to_cache: bool,
}

impl Default for OverlayCacheConfig {
    fn default() -> Self {
        Self { precompute: false, write_to_cache: true }
    }
}

/// Error returned when a state trie overlay cannot be built from the manager's current block set.
#[derive(Debug)]
pub(crate) struct StateTrieOverlayError {
    /// Requested in-memory tip hash.
    pub(crate) tip_hash: B256,
    /// Requested anchor hash.
    pub(crate) anchor_hash: B256,
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

/// A cached overlay, together with the number of the block it is anchored to.
///
/// An overlay covers `(anchor_number, tip_number]`. Recording where that range starts makes the
/// entry prunable without walking any chain: once the durable state trie frontier moves past the
/// anchor, the entry can never be requested again, because anchors are resolved at the frontier.
struct CachedOverlay<T> {
    /// Number of the block the overlay is anchored to.
    anchor_number: BlockNumber,
    /// The overlay itself, or the computation that will produce it.
    entry: OverlayCacheEntry<T>,
}

struct OverlayCache<T> {
    entries: Arc<DashMap<OverlayCacheKey, CachedOverlay<T>>>,
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

    fn retain(&self, mut keep: impl FnMut(&OverlayCacheKey, &CachedOverlay<T>) -> bool) {
        self.entries.retain(|key, cached| keep(key, cached))
    }

    /// Returns a ready entry without removing it from the cache.
    fn ready(&self, key: &OverlayCacheKey) -> Option<Arc<T>> {
        self.entries.get(key).and_then(|cached| match &cached.value().entry {
            OverlayCacheEntry::Ready(input) => Some(Arc::clone(input)),
            OverlayCacheEntry::Computing(_) => None,
        })
    }

    /// Removes and returns a ready entry.
    ///
    /// Transferring a parent entry lets `Arc::make_mut` extend it in place when no caller retains
    /// it. Keeping the cache entry would otherwise guarantee a clone.
    fn take_ready(&self, key: &OverlayCacheKey) -> Option<Arc<T>> {
        let (_, cached) = self
            .entries
            .remove_if(key, |_, cached| matches!(cached.entry, OverlayCacheEntry::Ready(_)))?;
        let OverlayCacheEntry::Ready(input) = cached.entry else { unreachable!() };
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
    ExtendCached { block: ExecutedBlock<N>, parent_input: Arc<T> },
    MergeBlocks(Vec<ExecutedBlock<N>>),
}

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
                target: "storage::overlay::manager",
                %anchor_hash,
                head = %block.recovered_block().hash(),
                "extending cached parent state trie overlay"
            );

            let mut parent_input = parent_input;
            extend_overlay(
                Arc::make_mut(&mut parent_input),
                &trie_data.sorted.hashed_state,
                &trie_data.sorted.trie_updates,
            );
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
        ComputeOverlayInput::ExtendCached { .. } => 1,
        ComputeOverlayInput::MergeBlocks(blocks) => blocks.len(),
    };
    let parent_overlay = matches!(&input, ComputeOverlayInput::ExtendCached { .. });
    tracing::Span::current().record("block_count", block_count);
    tracing::Span::current().record("parent_overlay", parent_overlay);

    let overlay = match input {
        ComputeOverlayInput::ExtendCached { block, parent_input } => {
            let mut parent_input = parent_input;
            Arc::make_mut(&mut parent_input).extend_block(&block);
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
    use crate::test_utils::TestOverlay;
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
        TestBlockBuilder::eth()
            .get_executed_blocks(1..4)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    impl TestOverlay {
        fn execution_overlay_for_parent(
            &self,
            parent_hash: B256,
            anchor_hash: B256,
        ) -> Result<Arc<ExecutionOverlay>, StateTrieOverlayError> {
            if parent_hash == anchor_hash {
                return Ok(Arc::new(ExecutionOverlay::default()))
            }
            let parent_state = self
                .state_for_hash(parent_hash)
                .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash })?;
            self.execution_overlay_for_block_state(
                &parent_state,
                anchor_hash,
                OverlayCacheConfig::default(),
            )
        }
    }

    fn overlay_for_parent(
        manager: &TestOverlay,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        let parent_state = manager
            .state_for_hash(parent_hash)
            .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash })?;
        manager.overlay_for_parent(&parent_state, anchor_hash, OverlayCacheConfig::default())
    }

    #[test]
    fn errors_for_unknown_parent() {
        let manager = TestOverlay::default();
        let parent = B256::random();
        let anchor = B256::random();

        let err = overlay_for_parent(&manager, parent, anchor).unwrap_err();

        assert_eq!(err.tip_hash, parent);
        assert_eq!(err.anchor_hash, anchor);
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
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
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
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
        let manager = TestOverlay::default();
        let anchor_hash = B256::with_last_byte(1);

        let overlay = manager.execution_overlay_for_parent(anchor_hash, anchor_hash).unwrap();

        assert!(overlay.accounts().is_empty());
        assert!(overlay.storage().is_empty());
        assert!(overlay.code_hashes().is_empty());
        assert!(overlay.block_hashes().is_empty());
    }

    #[test]
    fn promotes_ready_parent_overlays_to_the_child() {
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
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
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
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
            .and_then(|entry| match &entry.value().entry {
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
    fn does_not_cache_or_take_parent_overlays_for_unmanaged_blocks() {
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks[..2] {
            manager.insert_executed_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        let parent_state = manager.state_for_hash(parent_hash).unwrap();
        let child_state = BlockState::with_parent(blocks[2].clone(), Some(parent_state));
        let cache_config = OverlayCacheConfig { precompute: false, write_to_cache: false };

        overlay_for_parent(&manager, parent_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(parent_hash, anchor_hash).unwrap();

        let (_, state) =
            manager.overlay_for_parent(&child_state, anchor_hash, cache_config).unwrap();
        let execution = manager
            .execution_overlay_for_block_state(&child_state, anchor_hash, cache_config)
            .unwrap();

        assert_eq!(state.accounts.len(), 3);
        assert_eq!(execution.accounts().len(), 3);
        assert!(manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(!manager.state_trie_overlays.entries.contains_key(&child_key));
        assert!(manager.execution_overlays.entries.contains_key(&parent_key));
        assert!(!manager.execution_overlays.entries.contains_key(&child_key));
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn precomputes_execution_overlay_for_cached_parent() {
        let manager =
            TestOverlay::with_worker_pool(Arc::new(WorkerPool::new(1, "execution-overlay-test")));
        let blocks = test_blocks();
        let anchor_hash = blocks[0].recovered_block().parent_hash();

        manager.insert_executed_block(blocks[0].clone());
        manager
            .execution_overlay_for_parent(blocks[0].recovered_block().hash(), anchor_hash)
            .unwrap();

        manager.insert_executed_block(blocks[1].clone());
        let key = OverlayCacheKey { anchor_hash, tip_hash: blocks[1].recovered_block().hash() };
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !manager
            .execution_overlays
            .entries
            .get(&key)
            .is_some_and(|entry| matches!(entry.value().entry, OverlayCacheEntry::Ready(_)))
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
    fn precomputes_execution_overlay_after_anchor_advances() {
        let manager =
            TestOverlay::with_worker_pool(Arc::new(WorkerPool::new(1, "execution-overlay-test")));
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
        }
        let tip_hash = blocks[2].recovered_block().hash();
        let old_anchor = blocks[0].recovered_block().parent_hash();
        manager.execution_overlay_for_parent(tip_hash, old_anchor).unwrap();

        // The state/trie frontier advances to block 1. Block 2 may already be persisted at the
        // Finish frontier, but its execution state must remain in the overlay until trie
        // persistence.
        let new_anchor = blocks[0].recovered_block().hash();
        manager.remove_blocks_until(blocks[0].recovered_block().num_hash(), [new_anchor]);
        assert!(!manager
            .execution_overlays
            .entries
            .contains_key(&OverlayCacheKey { anchor_hash: old_anchor, tip_hash }));

        manager.precompute_execution_overlay(
            manager.state_for_hash(tip_hash).expect("tip is tracked in memory"),
            new_anchor,
        );

        let key = OverlayCacheKey { anchor_hash: new_anchor, tip_hash };
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let overlay = loop {
            if let Some(overlay) = manager.execution_overlays.ready(&key) {
                break overlay
            }
            assert!(std::time::Instant::now() < deadline, "new anchor overlay was not precomputed");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            overlay.block_hashes(),
            blocks[1..].iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(),
        );
        assert!(!overlay.accounts().contains_key(&Address::with_last_byte(1)));
        for id in 2..=3 {
            let address = Address::with_last_byte(id);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().nonce, id as u64);
            assert_eq!(overlay.storage()[&address][&U256::from(id)], U256::from(id));
        }
        let cached = manager.execution_overlay_for_parent(tip_hash, new_anchor).unwrap();
        assert!(Arc::ptr_eq(&overlay, &cached));
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn execution_overlay_precompute_does_not_wait_for_pending_entry() {
        let worker_pool = Arc::new(WorkerPool::new(1, "execution-overlay-pending-test"));
        let manager = TestOverlay::with_worker_pool(Arc::clone(&worker_pool));
        let block = test_blocks().remove(0);
        let anchor_hash = block.recovered_block().parent_hash();
        let tip_hash = block.recovered_block().hash();
        let state = manager.insert_executed_block(block);

        let waiter = Arc::new(OverlayWaiter::new());
        manager.execution_overlays.entries.insert(
            OverlayCacheKey { anchor_hash, tip_hash },
            CachedOverlay {
                anchor_number: 0,
                entry: OverlayCacheEntry::Computing(Arc::clone(&waiter)),
            },
        );

        let (tx, rx) = mpsc::channel();
        worker_pool.spawn(move || {
            manager
                .execution_overlay_for_parent_inner(
                    &state,
                    anchor_hash,
                    OverlayCacheConfig { precompute: true, write_to_cache: true },
                )
                .unwrap();
            tx.send(()).unwrap();
        });

        let completed = rx.recv_timeout(Duration::from_millis(100));
        waiter.finish(Arc::new(ExecutionOverlay::default()));
        assert!(completed.is_ok(), "execution overlay precompute waited for pending entry");
    }

    #[test]
    fn taking_sparse_trie_removes_it() {
        let manager = TestOverlay::default();
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
        let manager = TestOverlay::default();
        let block = test_blocks().remove(0);
        let parent_state = BlockState::new(block);
        let key = OverlayCacheKey {
            anchor_hash: parent_state.block_ref().recovered_block().parent_hash(),
            tip_hash: parent_state.hash(),
        };
        let waiter = Arc::new(OverlayWaiter::new());
        manager.state_trie_overlays.entries.insert(
            key,
            CachedOverlay {
                anchor_number: 0,
                entry: OverlayCacheEntry::Computing(Arc::clone(&waiter)),
            },
        );

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let res = manager
                .overlay_for_parent(&parent_state, key.anchor_hash, OverlayCacheConfig::default())
                .map(|(_, state)| state);
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
    fn prunes_fork_overlays_when_the_fork_is_removed() {
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
        }

        let fork = with_unique_state(
            &TestBlockBuilder::eth().get_executed_block_with_number(
                blocks[2].block_number(),
                blocks[1].recovered_block().hash(),
            ),
            9,
        );
        let fork_state = manager.insert_fork(fork);

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let key = OverlayCacheKey { anchor_hash, tip_hash: fork_state.hash() };
        manager
            .overlay_for_parent(&fork_state, anchor_hash, OverlayCacheConfig::default())
            .unwrap();
        manager
            .execution_overlay_for_block_state(
                &fork_state,
                anchor_hash,
                OverlayCacheConfig::default(),
            )
            .unwrap();
        assert!(manager.state_trie_overlays.entries.contains_key(&key));
        assert!(manager.execution_overlays.entries.contains_key(&key));

        // Pruning the fork goes through the store that owns it, and the manager drops the
        // overlays whose tip is gone.
        manager.remove_forks([fork_state.hash()], blocks[0].block_number());
        assert!(!manager.state_trie_overlays.entries.contains_key(&key));
        assert!(!manager.execution_overlays.entries.contains_key(&key));
    }

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = TestOverlay::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_executed_block(block.clone());
        }

        let original_anchor = blocks[0].recovered_block().parent_hash();
        overlay_for_parent(&manager, blocks[2].recovered_block().hash(), original_anchor).unwrap();
        manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .unwrap();

        manager.remove_blocks_until(
            blocks[1].recovered_block().num_hash(),
            [blocks[0].recovered_block().hash(), blocks[1].recovered_block().hash()],
        );

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
