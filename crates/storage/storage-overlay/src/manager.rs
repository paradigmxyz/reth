//! Flattened state trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`OverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened state trie overlays on demand.

use crate::{
    changeset_cache::compute_block_trie_updates, database_state_frontiers, ChangesetCache,
    OverlayBuilder,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use parking_lot::Mutex;
use reth_chain_state::{ExecutedBlock, PreservedSparseTrie};
use reth_errors::ProviderResult;
use reth_ethereum_primitives::EthPrimitives;
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
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
    collections::VecDeque,
    fmt,
    ops::RangeInclusive,
    sync::{Arc, OnceLock, Weak},
    time::Instant,
};
use tracing::{debug, trace};

/// Maximum number of completed flattened snapshots retained by the cache. Active readers retain
/// their own `Arc`s, and cache misses rebuild from the unchanged block graph.
const MAX_READY_OVERLAYS: usize = 4;

/// Manages flattened state trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph, changeset cache, and a cache of flattened state trie
/// overlays keyed by `(anchor_hash, tip_hash)`.
#[derive(Clone)]
pub struct OverlayManager<N: NodePrimitives = EthPrimitives> {
    blocks: Arc<DashMap<B256, ExecutedBlock<N>>>,
    overlays: Arc<DashMap<OverlayCacheKey, OverlayCacheEntry>>,
    /// Serializes publication and bounds cache ownership without retaining evicted snapshots.
    /// Always acquire this before an overlay map guard, never the reverse.
    ready_overlays: Arc<Mutex<VecDeque<(OverlayCacheKey, Weak<TrieInputSorted>)>>>,
    changeset_cache: ChangesetCache,
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

impl<N: NodePrimitives> Default for OverlayManager<N> {
    fn default() -> Self {
        Self {
            blocks: Default::default(),
            overlays: Default::default(),
            ready_overlays: Default::default(),
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            #[cfg(feature = "rayon")]
            worker_pool: None,
            metrics: Default::default(),
        }
    }
}

impl<N: NodePrimitives> std::fmt::Debug for OverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayManager")
            .field("blocks", &self.blocks.len())
            .field("overlays", &self.overlays.len())
            .finish()
    }
}

impl<N: NodePrimitives> OverlayManager<N> {
    /// Create a new [`OverlayManager`] backed by the given worker pool.
    #[cfg(feature = "rayon")]
    pub fn new(worker_pool: Arc<WorkerPool>) -> Self {
        Self {
            blocks: Default::default(),
            overlays: Default::default(),
            ready_overlays: Default::default(),
            changeset_cache: Default::default(),
            preserved_sparse_trie: Default::default(),
            worker_pool: Some(worker_pool),
            metrics: Default::default(),
        }
    }

    /// Creates an overlay builder for `parent_hash`.
    pub fn overlay_builder(&self, parent_hash: B256) -> OverlayBuilder<N> {
        OverlayBuilder::new(parent_hash, self.clone())
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
            + PruneCheckpointReader
            + StageCheckpointReader
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

        debug!(
            target: "storage::overlay::manager",
            %hash,
            %parent_hash,
            "inserted block into state trie overlay manager"
        );
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
            // Do not hold an overlay shard while consulting the block graph or dropping a
            // potentially large snapshot. Only remove the generation that was inspected.
            let candidates = self
                .overlays
                .iter()
                .map(|entry| (*entry.key(), entry.value().clone()))
                .collect::<Vec<_>>();
            for (key, candidate) in candidates {
                pruned_overlays += usize::from(self.prune_unreachable_overlay(key, &candidate));
            }
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

    fn prune_unreachable_overlay(
        &self,
        key: OverlayCacheKey,
        candidate: &OverlayCacheEntry,
    ) -> bool {
        if self.contains_hash(key.tip_hash, key.anchor_hash, key.anchor_hash) {
            return false
        }
        let removed =
            self.overlays.remove_if(&key, |_, current| current.same_generation(candidate));
        // The returned entry owns its snapshot and drops after the shard guard is released.
        removed.is_some()
    }

    /// Returns the flattened overlay from `anchor_hash` to `parent_hash`.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(tip_hash = %parent_hash, anchor_hash = %anchor_hash)
    )]
    pub(crate) fn overlay_for_parent(
        &self,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        debug!(
            target: "storage::overlay::manager",
            tip_hash = %parent_hash,
            %anchor_hash,
            "loading state trie overlay for parent"
        );
        let input = self.get_overlay(parent_hash, anchor_hash)?;
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
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
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();

        if let Some(entry) = self.overlays.get(&key).map(|entry| entry.value().clone()) {
            self.record_overlay_cache_reuse(&span);
            return Ok(match entry {
                OverlayCacheEntry::Ready(input) => input,
                OverlayCacheEntry::Computing(waiter) => waiter.wait(),
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
        }

        let action = match self.overlays.entry(key) {
            Entry::Occupied(entry) => {
                let entry = entry.get().clone();
                self.record_overlay_cache_reuse(&span);
                match entry {
                    OverlayCacheEntry::Ready(input) => CacheAction::Ready(input),
                    OverlayCacheEntry::Computing(waiter) => CacheAction::Wait(waiter),
                }
            }
            Entry::Vacant(entry) => {
                self.metrics.overlay_cache_fills.increment(1);
                let waiter = Arc::new(OverlayWaiter::new());
                entry.insert(OverlayCacheEntry::Computing(Arc::clone(&waiter)));
                CacheAction::Compute(waiter)
            }
        };

        match action {
            CacheAction::Ready(input) => Ok(input),
            CacheAction::Wait(waiter) => Ok(waiter.wait()),
            CacheAction::Compute(waiter) => {
                let input = self.compute_overlay(compute_input, anchor_hash, span);
                waiter.finish(Arc::clone(&input));

                self.publish_ready(key, &waiter, &input);

                Ok(input)
            }
        }
    }

    /// Publish a completed generation and evict the oldest completed cache ownership. Waiters
    /// and external readers remain valid even when their result is no longer cached.
    fn publish_ready(
        &self,
        key: OverlayCacheKey,
        waiter: &Arc<OverlayWaiter>,
        input: &Arc<TrieInputSorted>,
    ) {
        let removed = {
            let mut ready = self.ready_overlays.lock();
            let replaced = match self.overlays.entry(key) {
                Entry::Occupied(mut entry)
                    if matches!(entry.get(),
                    OverlayCacheEntry::Computing(existing) if Arc::ptr_eq(existing, waiter)) =>
                {
                    Some(entry.insert(OverlayCacheEntry::Ready(Arc::clone(input))))
                }
                _ => None,
            };
            // Pruning may have removed this waiter or replaced its generation during computation.
            let Some(replaced) = replaced else { return };
            let mut removed = vec![replaced];
            ready.push_back((key, Arc::downgrade(input)));
            while ready.len() > MAX_READY_OVERLAYS {
                let (old_key, generation) = ready.pop_front().expect("nonempty ready queue");
                if let Some((_, entry)) = self.overlays.remove_if(&old_key, |_, current| {
                    matches!(current, OverlayCacheEntry::Ready(snapshot)
                        if Arc::as_ptr(snapshot) == generation.as_ptr())
                }) {
                    removed.push(entry);
                }
            }
            removed
        };
        // Snapshot deallocation can be substantial; keep it outside both cache locks.
        drop(removed);
    }

    fn record_overlay_cache_reuse(&self, span: &tracing::Span) {
        self.metrics.overlay_cache_reuses.increment(1);
        span.record("cache_reused", true);
    }

    /// Returns every in-memory block in the chain whose tip is `parent_hash`.
    pub(crate) fn parent_chain(
        &self,
        parent_hash: B256,
    ) -> impl Iterator<Item = ExecutedBlock<N>> + '_ {
        let mut hash = parent_hash;
        std::iter::from_fn(move || {
            let block = self.blocks.get(&hash)?;
            hash = block.recovered_block().parent_hash();
            Some(block.clone())
        })
    }

    /// Returns true if `hash` is in the parent chain segment from `anchor_hash` inclusive to
    /// `parent_hash` inclusive.
    pub fn contains_hash(&self, parent_hash: B256, anchor_hash: B256, hash: B256) -> bool {
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
    fn same_generation(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ready(a), Self::Ready(b)) => Arc::ptr_eq(a, b),
            (Self::Computing(a), Self::Computing(b)) => Arc::ptr_eq(a, b),
            // Publication does not create a new generation: pruning may have captured the
            // waiter before this exact completed snapshot was published.
            (Self::Ready(input), Self::Computing(waiter)) => {
                waiter.input.get().is_some_and(|computed| Arc::ptr_eq(input, computed))
            }
            _ => false,
        }
    }

    fn ready(&self) -> Option<Arc<TrieInputSorted>> {
        match self {
            Self::Ready(input) => Some(Arc::clone(input)),
            Self::Computing(_) => None,
        }
    }
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
                target: "storage::overlay::manager",
                %anchor_hash,
                head = %block.recovered_block().hash(),
                "extending cached parent state trie overlay"
            );

            let mut overlay = parent_input.as_ref().clone();
            extend_overlay(
                &mut overlay,
                &trie_data.sorted.hashed_state,
                &trie_data.sorted.trie_updates,
            );
            overlay
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock, SparseTrie};
    use reth_ethereum_primitives::EthPrimitives;
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdatesSorted, ComputedTrieData, HashedPostState, HashedStorage};
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
        let manager = OverlayManager::<EthPrimitives>::default();
        let parent = B256::random();
        let anchor = B256::random();

        let err = manager.overlay_for_parent(parent, anchor).unwrap_err();

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

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = OverlayManager::default();
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

    fn ready_count(manager: &OverlayManager<EthPrimitives>) -> usize {
        manager
            .overlays
            .iter()
            .filter(|entry| matches!(entry.value(), OverlayCacheEntry::Ready(_)))
            .count()
    }

    fn many_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..13)
            .enumerate()
            .map(|(i, block)| with_unique_state(&block, i as u8 + 1))
            .collect()
    }

    fn assert_overlay_eq(actual: &TrieInputSorted, expected: &TrieInputSorted) {
        assert_eq!(actual.state, expected.state);
        assert_eq!(actual.nodes, expected.nodes);
    }

    #[test]
    fn ready_snapshots_are_bounded_and_eviction_preserves_readers_and_roots() {
        let manager = OverlayManager::default();
        let blocks = many_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }
        let anchor = blocks[0].recovered_block().parent_hash();
        let first = manager.get_overlay(blocks[0].recovered_block().hash(), anchor).unwrap();
        let first_weak = Arc::downgrade(&first);
        let first_expected = merge_blocks(vec![blocks[0].clone()]);
        for block in &blocks[1..] {
            manager.get_overlay(block.recovered_block().hash(), anchor).unwrap();
            assert!(ready_count(&manager) <= 4, "completed snapshots must be bounded");
        }
        assert_eq!(manager.blocks.len(), blocks.len(), "cache eviction must not prune blocks");
        assert_overlay_eq(&first, &first_expected);
        let rebuilt = manager.get_overlay(blocks[0].recovered_block().hash(), anchor).unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt), "historical tip should have been evicted");
        assert_overlay_eq(&rebuilt, &first_expected);
        assert_eq!(overlay_root(&rebuilt), overlay_root(&first));
        drop(first);
        assert!(first_weak.upgrade().is_none(), "eviction records must not retain snapshots");
    }

    fn overlay_root(input: &TrieInputSorted) -> B256 {
        use reth_provider::test_utils::create_test_provider_factory;
        use reth_trie::StateRoot;
        use reth_trie_db::{
            DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory,
            LegacyKeyAdapter,
        };
        type Root<'a, TX> = StateRoot<
            DatabaseTrieCursorFactory<&'a TX, LegacyKeyAdapter>,
            DatabaseHashedCursorFactory<&'a TX>,
        >;
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut input = input.clone();
        input.prefix_sets = input.state.construct_prefix_sets();
        let actual = Root::overlay_root_from_nodes(provider.tx_ref(), input.clone()).unwrap();
        let oracle = reth_trie::test_utils::state_root_prehashed(
            input.state.accounts.iter().filter_map(|(address, account)| {
                account.map(|account| {
                    (
                        *address,
                        (
                            account,
                            input
                                .state
                                .storages
                                .get(address)
                                .map(|storage| {
                                    storage
                                        .storage_slots_ref()
                                        .iter()
                                        .copied()
                                        .filter(|(_, value)| !value.is_zero())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default(),
                        ),
                    )
                })
            }),
        );
        assert_eq!(actual, oracle, "database overlay root must match independent triehash");
        actual
    }

    fn mixed_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        use reth_trie::Nibbles;
        let address = B256::with_last_byte(80);
        TestBlockBuilder::eth()
            .get_executed_blocks(1..9)
            .enumerate()
            .map(|(i, block)| {
                let account = (i != 4)
                    .then_some(Account { balance: U256::from(i + 1), ..Default::default() });
                let (wiped, slots) = match i {
                    0 => (false, vec![(1, 1), (2, 2)]),
                    1 => (false, vec![(1, 3)]),
                    2 => (false, vec![(2, 0)]),
                    3 => (true, vec![(3, 4)]),
                    4 => (true, vec![]),
                    5 => (false, vec![(4, 6)]),
                    _ => (false, vec![(1, i as u64 + 1)]),
                };
                let state = HashedPostState::default()
                    .with_accounts([(address, account)])
                    .with_storages([(
                        address,
                        HashedStorage::from_iter(
                            wiped,
                            slots.into_iter().map(|(slot, value)| {
                                (B256::with_last_byte(slot), U256::from(value))
                            }),
                        ),
                    )])
                    .into_sorted();
                // Nonempty node updates also participate in cached/uncached comparisons.
                let nodes = TrieUpdatesSorted::new(
                    vec![(Nibbles::from_nibbles([i as u8]), None)],
                    Default::default(),
                );
                ExecutedBlock::new(
                    Arc::clone(&block.recovered_block),
                    Arc::clone(&block.execution_output),
                    ComputedTrieData::new(Arc::new(state), Arc::new(nodes)),
                )
            })
            .collect()
    }

    #[test]
    fn evicted_mixed_state_and_forks_match_full_merge_at_multiple_anchors() {
        let manager = OverlayManager::default();
        let blocks = mixed_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }
        let anchor = blocks[0].recovered_block().parent_hash();
        for (i, block) in blocks.iter().enumerate() {
            let cached = manager.get_overlay(block.recovered_block().hash(), anchor).unwrap();
            let expected = merge_blocks(blocks[..=i].iter().rev().cloned().collect());
            assert_overlay_eq(&cached, &expected);
            assert_eq!(overlay_root(&cached), overlay_root(&expected));
        }
        // A different tip with the same parent exercises a fork, without modifying canonical data.
        let fork = with_unique_state(
            &TestBlockBuilder::eth()
                .get_executed_block_with_number(5, blocks[3].recovered_block().hash()),
            90,
        );
        manager.insert_block(fork.clone());
        let fork_state = manager.get_overlay(fork.recovered_block().hash(), anchor).unwrap();
        let fork_expected = merge_blocks(
            std::iter::once(fork.clone()).chain(blocks[..4].iter().rev().cloned()).collect(),
        );
        assert_overlay_eq(&fork_state, &fork_expected);
        assert_eq!(overlay_root(&fork_state), overlay_root(&fork_expected));
        for block in &blocks[..4] {
            let idx = (block.recovered_block().number() - 1) as usize;
            assert_overlay_eq(
                &manager.get_overlay(block.recovered_block().hash(), anchor).unwrap(),
                &merge_blocks(blocks[..=idx].iter().rev().cloned().collect()),
            );
        }
        let new_anchor = blocks[3].recovered_block().hash();
        let expected = merge_blocks(blocks[4..].iter().rev().cloned().collect());
        let before = manager.get_overlay(blocks[7].recovered_block().hash(), new_anchor).unwrap();
        assert_overlay_eq(&before, &expected);
        manager.remove_blocks(blocks[..4].iter().map(|block| block.recovered_block().hash()));
        let after = manager.get_overlay(blocks[7].recovered_block().hash(), new_anchor).unwrap();
        assert_overlay_eq(&after, &expected);
        assert!(manager.get_overlay(blocks[7].recovered_block().hash(), anchor).is_err());
        assert_overlay_eq(&before, &expected);
    }

    fn begin_computing(
        manager: &OverlayManager<EthPrimitives>,
        key: OverlayCacheKey,
    ) -> Arc<OverlayWaiter> {
        let waiter = Arc::new(OverlayWaiter::new());
        manager.overlays.insert(key, OverlayCacheEntry::Computing(Arc::clone(&waiter)));
        waiter
    }

    fn finish_computing(
        manager: &OverlayManager<EthPrimitives>,
        key: OverlayCacheKey,
        waiter: &Arc<OverlayWaiter>,
    ) -> Arc<TrieInputSorted> {
        let input = Arc::new(TrieInputSorted::default());
        waiter.finish(Arc::clone(&input));
        manager.publish_ready(key, waiter, &input);
        input
    }

    #[test]
    fn capacity_eviction_preserves_computing_waiters_and_stale_generations() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let key = OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(1) };
        let old_waiter = begin_computing(&manager, key);
        let (tx, rx) = mpsc::channel();
        let readers = (0..2)
            .map(|_| {
                let manager = manager.clone();
                let tx = tx.clone();
                thread::spawn(move || {
                    tx.send(manager.get_overlay(key.tip_hash, key.anchor_hash).unwrap()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        for id in 2..10 {
            let other =
                OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(id) };
            let waiter = begin_computing(&manager, other);
            finish_computing(&manager, other, &waiter);
        }
        assert!(matches!(
            manager.overlays.get(&key).unwrap().value(),
            OverlayCacheEntry::Computing(_)
        ));
        let first = finish_computing(&manager, key, &old_waiter);
        for _ in 0..2 {
            assert!(Arc::ptr_eq(&first, &rx.recv_timeout(Duration::from_secs(1)).unwrap()));
        }
        for reader in readers {
            reader.join().unwrap();
        }
        // Keep old snapshot alive so its stale Weak record is still upgradeable.
        manager.overlays.remove(&key);
        let replacement = begin_computing(&manager, key);
        let new = finish_computing(&manager, key, &replacement);
        manager.publish_ready(key, &old_waiter, &first);
        for id in 10..13 {
            let other =
                OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(id) };
            let waiter = begin_computing(&manager, other);
            finish_computing(&manager, other, &waiter);
        }
        assert!(
            Arc::ptr_eq(&new, &manager.get_overlay(key.tip_hash, key.anchor_hash).unwrap()),
            "stale record must not evict fresh generation"
        );
        assert_eq!(ready_count(&manager), 4);
    }

    #[test]
    fn pruned_producer_finishes_readers_without_overwriting_replacement() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let key = OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(1) };
        let old = begin_computing(&manager, key);
        manager.overlays.remove(&key);
        let new = begin_computing(&manager, key);
        let old_input = finish_computing(&manager, key, &old);
        assert!(Arc::ptr_eq(&old.wait(), &old_input));
        assert!(
            matches!(manager.overlays.get(&key).unwrap().value(), OverlayCacheEntry::Computing(waiter) if Arc::ptr_eq(waiter, &new))
        );
        let new_input = finish_computing(&manager, key, &new);
        assert!(Arc::ptr_eq(&new.wait(), &new_input));
        assert!(Arc::ptr_eq(
            &manager.get_overlay(key.tip_hash, key.anchor_hash).unwrap(),
            &new_input
        ));
    }

    #[test]
    fn concurrent_publications_finish_with_bounded_ready_ownership() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let (tx, rx) = mpsc::channel();
        let workers = (1..=8)
            .map(|id| {
                let manager = manager.clone();
                let barrier = Arc::clone(&barrier);
                let tx = tx.clone();
                let key =
                    OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(id) };
                let waiter = begin_computing(&manager, key);
                thread::spawn(move || {
                    barrier.wait();
                    finish_computing(&manager, key, &waiter);
                    tx.send(()).unwrap();
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for _ in 0..8 {
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(ready_count(&manager), 4);
        assert_eq!(manager.ready_overlays.lock().len(), 4);
    }
    #[test]
    fn pruning_captured_waiter_removes_its_completed_generation_only() {
        let manager = OverlayManager::<EthPrimitives>::default();
        let key = OverlayCacheKey { anchor_hash: B256::ZERO, tip_hash: B256::with_last_byte(1) };
        let waiter = begin_computing(&manager, key);
        let captured = manager.overlays.get(&key).unwrap().value().clone();
        // Deterministic interleaving: pruning captured Computing, then publication happened
        // before its reachability check and generation-checked removal.
        let external = finish_computing(&manager, key, &waiter);
        assert!(manager.prune_unreachable_overlay(key, &captured));
        assert!(Arc::ptr_eq(&external, &waiter.wait()));
        let replacement = begin_computing(&manager, key);
        assert!(!manager.prune_unreachable_overlay(key, &captured));
        let new = finish_computing(&manager, key, &replacement);
        assert!(!manager.prune_unreachable_overlay(key, &captured));
        assert!(Arc::ptr_eq(&new, &manager.get_overlay(key.tip_hash, key.anchor_hash).unwrap()));
    }
}
