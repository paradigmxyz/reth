//! State trie and execution overlays for in-memory blocks.
//!
//! An overlay holds the changes that a chain of in-memory blocks made on top of an anchor block, so
//! that the state as of an in-memory block can be read before the block is persisted.
//! [`OverlayManager`] builds reusable state trie and execution overlays on demand from the blocks
//! of the [`InMemoryState`] it was created with.

use crate::{
    changeset_cache::compute_block_trie_updates,
    database_state_frontiers,
    manager_metrics::{ExecutionOverlayMetrics, OverlayCacheMetrics, StateTrieOverlayMetrics},
    ChangesetCache, ExecutionOverlay, OverlayBuilder, OverlayError,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use parking_lot::Mutex;
use reth_chain_state::{
    BlockState, ExecutedBlock, InMemoryState, InMemoryStateBlockCounts, PreservedSparseTrie,
};
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
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Instant,
};
use tracing::{debug, trace};

/// Manages state trie and execution overlays for in-memory blocks.
///
/// The manager builds overlays from the blocks of the [`InMemoryState`] it was created with. It
/// owns the changeset cache and the overlay caches, which are keyed by `(anchor_hash, tip_hash)`.
///
/// The overlay caches need no upkeep: after blocks were removed from the in-memory state, the
/// manager drops the cached overlays that cannot be built from the remaining blocks before it
/// serves the next overlay.
///
/// Clones share the in-memory state and all caches.
#[derive(Clone)]
pub struct OverlayManager<N: NodePrimitives = EthPrimitives> {
    in_memory_state: InMemoryState<N>,
    state_trie_overlays: OverlayCache<TrieInputSorted>,
    execution_overlays: OverlayCache<ExecutionOverlay>,
    /// The removal generation of `in_memory_state` the overlay caches were pruned at last.
    pruned_generation: Arc<AtomicU64>,
    changeset_cache: ChangesetCache,
    preserved_sparse_trie: Arc<Mutex<Option<PreservedSparseTrie>>>,
    #[cfg(feature = "rayon")]
    worker_pool: Option<Arc<WorkerPool>>,
    metrics: StateTrieOverlayMetrics,
    execution_metrics: ExecutionOverlayMetrics,
}

impl<N: NodePrimitives> std::fmt::Debug for OverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let InMemoryStateBlockCounts { canonical, non_canonical } =
            self.in_memory_state.block_counts();
        f.debug_struct("OverlayManager")
            .field("canonical_blocks", &canonical)
            .field("non_canonical_blocks", &non_canonical)
            .field("state_trie_overlays", &self.state_trie_overlays.len())
            .field("execution_overlays", &self.execution_overlays.len())
            .finish()
    }
}

impl<N: NodePrimitives> OverlayManager<N> {
    /// Creates a manager that builds overlays from the blocks of `in_memory_state` and computes
    /// them on the calling thread.
    pub fn new(in_memory_state: InMemoryState<N>) -> Self {
        // No overlay is cached yet, so there is nothing to prune for the removals so far.
        let pruned_generation = Arc::new(AtomicU64::new(in_memory_state.removal_generation()));
        Self {
            in_memory_state,
            state_trie_overlays: Default::default(),
            execution_overlays: Default::default(),
            pruned_generation,
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            #[cfg(feature = "rayon")]
            worker_pool: None,
            metrics: Default::default(),
            execution_metrics: Default::default(),
        }
    }

    /// Creates a manager like [`Self::new`] that computes cached overlays on `worker_pool`.
    #[cfg(feature = "rayon")]
    pub fn with_worker_pool(
        in_memory_state: InMemoryState<N>,
        worker_pool: Arc<WorkerPool>,
    ) -> Self {
        Self { worker_pool: Some(worker_pool), ..Self::new(in_memory_state) }
    }

    /// Returns the in-memory state the overlays are built from.
    pub const fn in_memory_state(&self) -> &InMemoryState<N> {
        &self.in_memory_state
    }

    /// Creates an overlay builder for `parent_hash`.
    ///
    /// This looks up the in-memory chain ending at `parent_hash`, canonical or not. Prefer
    /// [`Self::overlay_builder_for_state`] whenever the caller already holds the chain.
    pub fn overlay_builder(&self, parent_hash: B256) -> OverlayBuilder<N> {
        OverlayBuilder::new(
            parent_hash,
            self.in_memory_state.state_by_hash(parent_hash),
            self.clone(),
        )
    }

    /// Creates an overlay builder for an already materialized in-memory chain.
    ///
    /// The chain tip is used as the parent hash, so no block lookup is performed. The builder
    /// only reads `state`, which makes it safe to share the same [`BlockState`] with other
    /// holders.
    pub fn overlay_builder_for_state(&self, state: Arc<BlockState<N>>) -> OverlayBuilder<N> {
        OverlayBuilder::new(state.hash(), Some(state), self.clone())
    }

    pub(crate) const fn changeset_cache(&self) -> &ChangesetCache {
        &self.changeset_cache
    }

    /// Gets or computes cached changesets for an inclusive block range.
    ///
    /// # Errors
    ///
    /// Besides database errors, an [`OverlayError`] as [`ProviderError::Other`] if an overlay the
    /// computation needs cannot be built from in-memory blocks.
    /// [`ProviderError::downcast_other_ref`] returns it.
    ///
    /// [`ProviderError::Other`]: reth_errors::ProviderError::Other
    /// [`ProviderError::downcast_other_ref`]: reth_errors::ProviderError::downcast_other_ref
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
    ///
    /// # Errors
    ///
    /// Besides database errors, an [`OverlayError`] as [`ProviderError::Other`] if an overlay the
    /// computation needs cannot be built from in-memory blocks.
    /// [`ProviderError::downcast_other_ref`] returns it.
    ///
    /// [`ProviderError::Other`]: reth_errors::ProviderError::Other
    /// [`ProviderError::downcast_other_ref`]: reth_errors::ProviderError::downcast_other_ref
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

    /// Optimistically extends every cached execution overlay that ends at the parent of the
    /// tracked block `block_hash` by that block.
    ///
    /// This returns without waiting. Nothing is scheduled if the block is not tracked, if no
    /// overlay ending at its parent is cached, or without a worker pool.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(block_hash = %block_hash, parent_hash = tracing::field::Empty)
    )]
    pub fn precompute_child_execution_overlays(&self, block_hash: B256) {
        let Some(state) = self.in_memory_state.state_by_hash(block_hash) else { return };
        let parent_hash = state.parent_hash();
        tracing::Span::current().record("parent_hash", tracing::field::display(parent_hash));

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
            %block_hash,
            %parent_hash,
            cached_parent_overlays = cached_parent_overlays.len(),
            "precomputing execution overlays for the child of cached overlays"
        );

        #[cfg(not(feature = "rayon"))]
        let _ = cached_parent_overlays;

        // Optimistically and asynchronously flatten an execution overlay for the block on top of
        // each cached overlay of its parent.
        #[cfg(feature = "rayon")]
        for anchor_hash in cached_parent_overlays {
            self.precompute_execution_overlay(block_hash, anchor_hash);
        }
    }

    /// Optimistically computes an execution overlay from `anchor_hash` to `tip_hash`.
    ///
    /// This returns without waiting for computation. No work is scheduled without a worker pool
    /// or when the tip is already persisted at the anchor. A concurrent persistence or reorg may
    /// make the requested range unavailable, in which case the background task skips it.
    #[cfg(feature = "rayon")]
    pub fn precompute_execution_overlay(&self, tip_hash: B256, anchor_hash: B256) {
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
            if let Err(err) = manager.precompute_execution_overlay_for_parent(tip_hash, anchor_hash) {
                debug!(target: "storage::overlay::manager", %err, "Skipping execution overlay precompute");
            }
        });
    }

    /// Drops the cached overlays that can no longer be requested, if blocks were removed from
    /// the in-memory state since this was done last.
    fn prune_overlays_if_blocks_removed(&self) {
        let generation = self.in_memory_state.removal_generation();
        if self.pruned_generation.swap(generation, Ordering::Relaxed) != generation {
            self.prune_unreachable_overlays();
        }
    }

    /// Drops the cached overlays whose tip is not tracked by the in-memory state, or whose anchor
    /// is not on the in-memory chain that ends at the tip.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(pruned_overlays = tracing::field::Empty)
    )]
    fn prune_unreachable_overlays(&self) {
        let overlays_before = self.state_trie_overlays.len() + self.execution_overlays.len();
        let is_reachable = |key: &OverlayCacheKey| {
            self.in_memory_state
                .state_by_hash(key.tip_hash)
                .is_some_and(|state| state.chain_hashes().any(|hash| hash == key.anchor_hash))
        };
        self.state_trie_overlays.retain(|key, _| is_reachable(key));
        self.execution_overlays.retain(|key, _| is_reachable(key));
        let remaining_overlays = self.state_trie_overlays.len() + self.execution_overlays.len();
        let pruned_overlays = overlays_before.saturating_sub(remaining_overlays);
        tracing::Span::current().record("pruned_overlays", pruned_overlays);
        debug!(
            target: "storage::overlay::manager",
            pruned_overlays,
            remaining_overlays,
            "pruned overlays that can no longer be built from the in-memory state"
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
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), OverlayError> {
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
                |input, span| {
                    self.compute_state_trie_overlay(
                        input,
                        anchor_hash,
                        span,
                        cache_config.write_to_cache,
                    )
                },
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
    ) -> Result<Arc<ExecutionOverlay>, OverlayError> {
        Ok(self
            .execution_overlay_for_parent_inner(parent_state, anchor_hash, cache_config)?
            .expect("required overlay lookup cannot skip an in-progress computation"))
    }

    #[cfg(feature = "rayon")]
    fn precompute_execution_overlay_for_parent(
        &self,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(), OverlayError> {
        let parent_state = self
            .in_memory_state
            .state_by_hash(parent_hash)
            .ok_or(OverlayError::TipNotInMemory { tip_hash: parent_hash })?;
        self.execution_overlay_for_parent_inner(
            &parent_state,
            anchor_hash,
            OverlayCacheConfig { precompute: true, write_to_cache: true },
        )
        .map(drop)
    }

    fn execution_overlay_for_parent_inner(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
    ) -> Result<Option<Arc<ExecutionOverlay>>, OverlayError> {
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
            |input, span| {
                self.compute_execution_overlay(
                    input,
                    anchor_hash,
                    span,
                    cache_config.write_to_cache,
                )
            },
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
    ) -> Result<Option<Arc<T>>, OverlayError>
    where
        M: OverlayCacheMetrics,
    {
        self.prune_overlays_if_blocks_removed();

        let tip_hash = parent_state.hash();
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();
        if let Some(entry) = cache.entries.get(&key).map(|entry| entry.value().clone()) {
            metrics.record_cache_reuse();
            span.record("cache_reused", true);
            return match entry {
                OverlayCacheEntry::Ready(input) => Ok(Some(input)),
                OverlayCacheEntry::Computing(_) if cache_config.precompute => Ok(None),
                OverlayCacheEntry::Computing(waiter) => Ok(Some(waiter.wait(metrics))),
            }
        }
        span.record("cache_reused", false);

        // Resolve the block path and any cached parent overlay before locking the child entry.
        let mut blocks = Self::blocks_from_parent_state(parent_state, anchor_hash)?;
        span.record("block_count", blocks.len());

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

        let parent_hash = parent_state.parent_hash();
        cache.retain(|sibling_key, entry| {
            sibling_key.tip_hash == tip_hash ||
                !matches!(entry, OverlayCacheEntry::Ready(_)) ||
                self.in_memory_state
                    .state_by_hash(sibling_key.tip_hash)
                    .is_none_or(|state| state.parent_hash() != parent_hash)
        });

        let action = match cache.entries.entry(key) {
            Entry::Occupied(entry) => {
                let entry = entry.get().clone();
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
                entry.insert(OverlayCacheEntry::Computing(Arc::clone(&waiter)));
                CacheAction::Compute(waiter)
            }
        };

        match action {
            CacheAction::Ready(input) => Ok(Some(input)),
            CacheAction::Wait(waiter) => Ok(Some(waiter.wait(metrics))),
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

    fn blocks_from_parent_state(
        parent_state: &BlockState<N>,
        anchor_hash: B256,
    ) -> Result<Vec<ExecutedBlock<N>>, OverlayError> {
        let tip_hash = parent_state.hash();
        let mut hash = tip_hash;
        let mut blocks = Vec::new();
        for state in parent_state.chain() {
            if state.hash() != hash {
                return Err(OverlayError::AnchorNotReached { tip_hash, anchor_hash })
            }
            hash = state.parent_hash();
            blocks.push(state.block());
            if hash == anchor_hash {
                return Ok(blocks)
            }
        }
        Err(OverlayError::AnchorNotReached { tip_hash, anchor_hash })
    }

    fn compute_state_trie_overlay(
        &self,
        compute_input: ComputeOverlayInput<N, TrieInputSorted>,
        anchor_hash: B256,
        _span: tracing::Span,
        write_to_cache: bool,
    ) -> TrieInputSorted {
        if !write_to_cache {
            return compute_overlay(compute_input, anchor_hash, &self.metrics)
        }

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
        write_to_cache: bool,
    ) -> ExecutionOverlay {
        if !write_to_cache {
            return compute_execution_overlay_inner(
                compute_input,
                anchor_hash,
                &self.execution_metrics,
            )
        }

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

    /// Returns a ready entry without removing it from the cache.
    fn ready(&self, key: &OverlayCacheKey) -> Option<Arc<T>> {
        self.entries.get(key).and_then(|entry| match entry.value() {
            OverlayCacheEntry::Ready(input) => Some(Arc::clone(input)),
            OverlayCacheEntry::Computing(_) => None,
        })
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

    fn wait(&self, metrics: &impl OverlayCacheMetrics) -> Arc<T> {
        let start = Instant::now();
        let input = self.input.wait();
        metrics.record_wait_duration(start.elapsed());
        Arc::clone(input)
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
    use crate::test_utils::overlay_manager_with_writer;
    use alloy_primitives::{map::HashMap, Address, U256};
    use reth_chain_state::{
        test_utils::TestBlockBuilder, ExecutedBlock, InMemoryStateWriter, NewCanonicalChain,
        SparseTrie,
    };
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

    impl OverlayManager {
        fn execution_overlay_for_parent(
            &self,
            parent_hash: B256,
            anchor_hash: B256,
        ) -> Result<Arc<ExecutionOverlay>, OverlayError> {
            if parent_hash == anchor_hash {
                return Ok(Arc::new(ExecutionOverlay::default()))
            }
            let parent_state = self
                .in_memory_state()
                .state_by_hash(parent_hash)
                .ok_or(OverlayError::TipNotInMemory { tip_hash: parent_hash })?;
            self.execution_overlay_for_block_state(
                &parent_state,
                anchor_hash,
                OverlayCacheConfig::default(),
            )
        }
    }

    fn overlay_for_parent(
        manager: &OverlayManager,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), OverlayError> {
        let parent_state = manager
            .in_memory_state()
            .state_by_hash(parent_hash)
            .ok_or(OverlayError::TipNotInMemory { tip_hash: parent_hash })?;
        manager.overlay_for_parent(&parent_state, anchor_hash, OverlayCacheConfig::default())
    }

    /// Creates a manager over an in-memory state that stays empty.
    fn manager() -> OverlayManager {
        OverlayManager::new(InMemoryState::read_only())
    }

    /// Makes `blocks` the canonical chain of the in-memory state.
    fn commit(
        in_memory_state_writer: &mut InMemoryStateWriter,
        blocks: &[ExecutedBlock<EthPrimitives>],
    ) {
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.to_vec() });
    }

    /// Removes the canonical blocks up to and including `persisted` from the in-memory state.
    fn trim(
        in_memory_state_writer: &mut InMemoryStateWriter,
        persisted: &ExecutedBlock<EthPrimitives>,
    ) {
        let persisted = persisted.recovered_block().num_hash();
        in_memory_state_writer.remove_canonical_until(persisted.hash, persisted.number);
    }

    /// Caches an empty state trie overlay and an empty execution overlay for `key`.
    fn cache_empty_overlays(manager: &OverlayManager, key: OverlayCacheKey) {
        manager
            .state_trie_overlays
            .entries
            .insert(key, OverlayCacheEntry::Ready(Arc::new(TrieInputSorted::default())));
        manager
            .execution_overlays
            .entries
            .insert(key, OverlayCacheEntry::Ready(Arc::new(ExecutionOverlay::default())));
    }

    /// Returns `true` if a state trie overlay and an execution overlay are cached for `key`, and
    /// `false` if neither is.
    fn caches_overlays(manager: &OverlayManager, key: OverlayCacheKey) -> bool {
        let cached = manager.state_trie_overlays.entries.contains_key(&key);
        assert_eq!(manager.execution_overlays.entries.contains_key(&key), cached);
        cached
    }

    #[test]
    fn errors_for_unknown_parent() {
        let manager = manager();
        let parent = B256::random();
        let anchor = B256::random();

        let err = overlay_for_parent(&manager, parent, anchor).unwrap_err();

        assert_eq!(err, OverlayError::TipNotInMemory { tip_hash: parent });
    }

    #[test]
    fn errors_for_anchor_off_the_tip_chain() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }
        let tip_hash = blocks[2].recovered_block().hash();
        let anchor_hash = B256::random();
        let expected = OverlayError::AnchorNotReached { tip_hash, anchor_hash };

        assert_eq!(overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap_err(), expected);
        assert_eq!(
            manager.execution_overlay_for_parent(tip_hash, anchor_hash).unwrap_err(),
            expected
        );
        assert!(manager.state_trie_overlays.entries.is_empty());
        assert!(manager.execution_overlays.entries.is_empty());
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
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
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
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
    fn computing_sibling_evicts_ready_cached_overlays() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let mut builder = TestBlockBuilder::eth();
        let anchor_hash = B256::random();
        let parent = builder.get_executed_block_with_number(1, anchor_hash);
        let parent_hash = parent.recovered_block().hash();
        let first = builder.get_executed_block_with_number(2, parent_hash);
        let sibling = builder.get_executed_block_with_number(2, parent_hash);
        let sibling_hash = sibling.recovered_block().hash();
        let first_key = OverlayCacheKey { anchor_hash, tip_hash: first.recovered_block().hash() };

        in_memory_state_writer.insert_executed(parent);
        in_memory_state_writer.insert_executed(first);
        in_memory_state_writer.insert_executed(sibling);
        manager
            .state_trie_overlays
            .entries
            .insert(first_key, OverlayCacheEntry::Ready(Arc::new(TrieInputSorted::default())));
        manager
            .execution_overlays
            .entries
            .insert(first_key, OverlayCacheEntry::Ready(Arc::new(ExecutionOverlay::default())));

        overlay_for_parent(&manager, sibling_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(sibling_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&first_key));
        assert!(!manager.execution_overlays.entries.contains_key(&first_key));
    }

    #[test]
    fn execution_overlay_for_parent_at_anchor_is_empty() {
        let manager = manager();
        let anchor_hash = B256::with_last_byte(1);

        let overlay = manager.execution_overlay_for_parent(anchor_hash, anchor_hash).unwrap();

        assert!(overlay.accounts().is_empty());
        assert!(overlay.storage().is_empty());
        assert!(overlay.code_hashes().is_empty());
        assert!(overlay.block_hashes().is_empty());
    }

    #[test]
    fn promotes_ready_parent_overlays_to_the_child() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
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
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
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
    fn does_not_cache_or_take_parent_overlays_for_unmanaged_blocks() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks[..2] {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        let parent_state = manager.in_memory_state().state_by_hash(parent_hash).unwrap();
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
    fn uncached_overlays_do_not_use_worker_pool() {
        let worker_pool = Arc::new(WorkerPool::new(1, "uncached-overlay-test"));
        let manager =
            OverlayManager::with_worker_pool(InMemoryState::read_only(), Arc::clone(&worker_pool));
        let block = test_blocks().remove(0);
        let anchor_hash = block.recovered_block().parent_hash();
        let parent_state = BlockState::new(block);
        let cache_config = OverlayCacheConfig { precompute: false, write_to_cache: false };

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        worker_pool.spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();

        let (completed_tx, completed_rx) = mpsc::channel();
        let task = thread::spawn(move || {
            let execution =
                manager.execution_overlay_for_block_state(&parent_state, anchor_hash, cache_config);
            let state = manager.overlay_for_parent(&parent_state, anchor_hash, cache_config);
            completed_tx.send((execution, state)).unwrap();
        });

        let completed = completed_rx.recv_timeout(Duration::from_millis(100));
        release_tx.send(()).unwrap();
        task.join().unwrap();

        assert!(completed.is_ok(), "uncached overlay used the worker pool");
        let (execution, state) = completed.unwrap();
        assert!(execution.is_ok());
        assert!(state.is_ok());
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn precomputes_execution_overlay_for_cached_parent() {
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let manager = OverlayManager::with_worker_pool(
            in_memory_state,
            Arc::new(WorkerPool::new(1, "execution-overlay-test")),
        );
        let blocks = test_blocks();
        let anchor_hash = blocks[0].recovered_block().parent_hash();

        in_memory_state_writer.insert_executed(blocks[0].clone());
        manager
            .execution_overlay_for_parent(blocks[0].recovered_block().hash(), anchor_hash)
            .unwrap();

        in_memory_state_writer.insert_executed(blocks[1].clone());
        manager.precompute_child_execution_overlays(blocks[1].recovered_block().hash());
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
    fn precomputes_execution_overlay_after_anchor_advances() {
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let manager = OverlayManager::with_worker_pool(
            in_memory_state,
            Arc::new(WorkerPool::new(1, "execution-overlay-test")),
        );
        let blocks = test_blocks();
        commit(&mut in_memory_state_writer, &blocks);
        let tip_hash = blocks[2].recovered_block().hash();
        let old_anchor = blocks[0].recovered_block().parent_hash();
        manager.execution_overlay_for_parent(tip_hash, old_anchor).unwrap();

        // The state/trie frontier advances to block 1. Block 2 may already be persisted at the
        // Finish frontier, but its execution state must remain in the overlay until trie
        // persistence.
        let new_anchor = blocks[0].recovered_block().hash();
        trim(&mut in_memory_state_writer, &blocks[0]);

        manager.precompute_execution_overlay(tip_hash, new_anchor);

        let key = OverlayCacheKey { anchor_hash: new_anchor, tip_hash };
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let overlay = loop {
            if let Some(overlay) = manager.execution_overlays.ready(&key) {
                break overlay
            }
            assert!(std::time::Instant::now() < deadline, "new anchor overlay was not precomputed");
            thread::sleep(Duration::from_millis(10));
        };
        // The overlay request of the precompute dropped the overlay anchored at the removed
        // block's parent.
        assert!(!manager
            .execution_overlays
            .entries
            .contains_key(&OverlayCacheKey { anchor_hash: old_anchor, tip_hash }));
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
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let manager = OverlayManager::with_worker_pool(in_memory_state, Arc::clone(&worker_pool));
        let block = test_blocks().remove(0);
        let anchor_hash = block.recovered_block().parent_hash();
        let tip_hash = block.recovered_block().hash();
        in_memory_state_writer.insert_executed(block);

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

    #[cfg(feature = "rayon")]
    #[test]
    fn execution_overlay_precompute_errors_for_untracked_tip() {
        let manager = OverlayManager::<EthPrimitives>::with_worker_pool(
            InMemoryState::read_only(),
            Arc::new(WorkerPool::new(1, "execution-overlay-test")),
        );
        let tip_hash = B256::random();

        assert_eq!(
            manager.precompute_execution_overlay_for_parent(tip_hash, B256::random()),
            Err(OverlayError::TipNotInMemory { tip_hash })
        );
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn does_not_precompute_child_execution_overlays_for_untracked_block() {
        let worker_pool = Arc::new(WorkerPool::new(1, "execution-overlay-test"));
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let manager = OverlayManager::with_worker_pool(in_memory_state, Arc::clone(&worker_pool));
        let blocks = test_blocks();
        let parent_key = OverlayCacheKey {
            anchor_hash: blocks[0].recovered_block().parent_hash(),
            tip_hash: blocks[0].recovered_block().hash(),
        };
        in_memory_state_writer.insert_executed(blocks[0].clone());
        manager
            .execution_overlays
            .entries
            .insert(parent_key, OverlayCacheEntry::Ready(Arc::new(ExecutionOverlay::default())));

        // The overlay of the parent is cached, but the child is not tracked.
        manager.precompute_child_execution_overlays(blocks[1].recovered_block().hash());

        assert!(!worker_pool.is_initialized(), "scheduled a precompute for an untracked block");
        assert_eq!(manager.execution_overlays.len(), 1);
        assert!(manager.execution_overlays.entries.contains_key(&parent_key));
    }

    #[test]
    fn prune_keeps_overlays_anchored_on_the_tip_chain() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let tip_hash = blocks[2].recovered_block().hash();
        let chain_hashes = manager
            .in_memory_state()
            .state_by_hash(tip_hash)
            .unwrap()
            .chain_hashes()
            .collect::<Vec<_>>();
        assert_eq!(
            chain_hashes,
            blocks
                .iter()
                .rev()
                .map(|block| block.recovered_block().hash())
                .chain([anchor_hash])
                .collect::<Vec<_>>()
        );

        let unknown_key = OverlayCacheKey { anchor_hash: B256::random(), tip_hash };
        cache_empty_overlays(&manager, unknown_key);
        for &anchor_hash in &chain_hashes {
            cache_empty_overlays(&manager, OverlayCacheKey { anchor_hash, tip_hash });
        }

        manager.prune_unreachable_overlays();

        for anchor_hash in chain_hashes {
            assert!(caches_overlays(&manager, OverlayCacheKey { anchor_hash, tip_hash }));
        }
        assert!(!caches_overlays(&manager, unknown_key));
    }

    #[test]
    fn prune_drops_overlays_anchored_below_the_tip_chain() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        // The chain of the tip ends at block 1, which is not tracked.
        in_memory_state_writer.insert_executed(blocks[2].clone());

        let tip_hash = blocks[2].recovered_block().hash();
        let anchor_key =
            OverlayCacheKey { anchor_hash: blocks[1].recovered_block().hash(), tip_hash };
        let below_anchor_key =
            OverlayCacheKey { anchor_hash: blocks[0].recovered_block().hash(), tip_hash };
        cache_empty_overlays(&manager, anchor_key);
        cache_empty_overlays(&manager, below_anchor_key);

        manager.prune_unreachable_overlays();

        assert!(caches_overlays(&manager, anchor_key));
        assert!(!caches_overlays(&manager, below_anchor_key));
    }

    #[test]
    fn prune_drops_overlays_with_unknown_anchor_or_untracked_tip() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        for block in &blocks[..2] {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let unknown_anchor_key = OverlayCacheKey {
            anchor_hash: B256::random(),
            tip_hash: blocks[1].recovered_block().hash(),
        };
        let untracked_tip_key = OverlayCacheKey {
            anchor_hash: blocks[0].recovered_block().parent_hash(),
            tip_hash: blocks[2].recovered_block().hash(),
        };
        cache_empty_overlays(&manager, unknown_anchor_key);
        cache_empty_overlays(&manager, untracked_tip_key);

        manager.prune_unreachable_overlays();

        assert!(!caches_overlays(&manager, unknown_anchor_key));
        assert!(!caches_overlays(&manager, untracked_tip_key));
    }

    #[test]
    fn taking_sparse_trie_removes_it() {
        let manager = manager();
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
        let manager = manager();
        let block = test_blocks().remove(0);
        let parent_state = BlockState::new(block);
        let key = OverlayCacheKey {
            anchor_hash: parent_state.parent_hash(),
            tip_hash: parent_state.hash(),
        };
        let waiter = Arc::new(OverlayWaiter::new());
        manager
            .state_trie_overlays
            .entries
            .insert(key, OverlayCacheEntry::Computing(Arc::clone(&waiter)));

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
    fn prunes_cached_overlays_after_trimming_blocks() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        commit(&mut in_memory_state_writer, &blocks);

        let original_anchor = blocks[0].recovered_block().parent_hash();
        overlay_for_parent(&manager, blocks[2].recovered_block().hash(), original_anchor).unwrap();
        manager
            .execution_overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .unwrap();

        let tip_hash = blocks[2].recovered_block().hash();
        let original_key = OverlayCacheKey { anchor_hash: original_anchor, tip_hash };
        assert!(caches_overlays(&manager, original_key));

        // Removing blocks through the writer is all it takes: the next overlay request drops the
        // overlays anchored below the removed blocks instead of serving them.
        trim(&mut in_memory_state_writer, &blocks[1]);

        let anchor_hash = blocks[1].recovered_block().hash();
        let unreachable = OverlayError::AnchorNotReached { tip_hash, anchor_hash: original_anchor };
        assert_eq!(
            overlay_for_parent(&manager, tip_hash, original_anchor).unwrap_err(),
            unreachable
        );
        assert!(!caches_overlays(&manager, original_key));
        assert_eq!(
            manager.execution_overlay_for_parent(tip_hash, original_anchor).unwrap_err(),
            unreachable
        );
        assert!(!caches_overlays(&manager, original_key));

        let (_, state) = overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 1);
        let execution = manager.execution_overlay_for_parent(tip_hash, anchor_hash).unwrap();
        assert_eq!(execution.accounts().len(), 1);
    }

    #[test]
    fn prunes_cached_overlays_of_pruned_forks() {
        let (manager, mut in_memory_state_writer) = overlay_manager_with_writer();
        let blocks = test_blocks();
        commit(&mut in_memory_state_writer, &blocks);
        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let tip_hash = blocks[2].recovered_block().hash();
        let fork = with_unique_state(
            &TestBlockBuilder::eth()
                .get_executed_block_with_number(2, blocks[0].recovered_block().hash()),
            9,
        );
        let fork_hash = fork.recovered_block().hash();
        in_memory_state_writer.insert_executed(fork);

        overlay_for_parent(&manager, fork_hash, anchor_hash).unwrap();
        let (_, tip_state) = overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap();
        let fork_key = OverlayCacheKey { anchor_hash, tip_hash: fork_hash };
        let tip_key = OverlayCacheKey { anchor_hash, tip_hash };
        assert!(manager.state_trie_overlays.entries.contains_key(&fork_key));

        // Finalizing block 2 removes the fork at the same height. The next overlay request drops
        // the overlays of the fork and keeps the others.
        assert_eq!(
            in_memory_state_writer
                .prune_conflicting_with_finalized(blocks[1].recovered_block().num_hash()),
            1
        );
        let (_, cached_tip_state) = overlay_for_parent(&manager, tip_hash, anchor_hash).unwrap();

        assert!(Arc::ptr_eq(&tip_state, &cached_tip_state));
        assert!(manager.state_trie_overlays.entries.contains_key(&tip_key));
        assert!(!manager.state_trie_overlays.entries.contains_key(&fork_key));
        assert_eq!(
            overlay_for_parent(&manager, fork_hash, anchor_hash).unwrap_err(),
            OverlayError::TipNotInMemory { tip_hash: fork_hash }
        );
    }
}
