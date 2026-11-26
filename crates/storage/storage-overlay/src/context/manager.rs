//! Caches state trie and execution overlays for in-memory blocks.

use crate::{
    manager_metrics::{
        ExecutionOverlayMetrics, OverlayBuilderMetrics, OverlayCacheMetrics,
        OverlayStateProviderFactoryMetrics, StateTrieOverlayMetrics,
    },
    overlay::{
        blocks_from_parent_state, compute_execution_overlay, compute_state_trie_overlay,
        compute_with_worker_pool, live_state_reaches, ComputeOverlayInput,
    },
    ChangesetCache, ExecutionOverlay, OverlayError,
};
use alloy_primitives::B256;
use reth_chain_state::{BlockState, InMemoryState};
use reth_primitives_traits::{
    dashmap::{mapref::entry::Entry, DashMap},
    AlloyBlockHeader, NodePrimitives,
};
use reth_trie::{
    proof_v2::ProofCursorMetrics, updates::TrieUpdatesSorted, HashedPostStateSorted,
    TrieInputSorted,
};
use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Instant,
};
use tracing::debug;

#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;

/// Maintains the overlay caches for one [`crate::OverlayContext`].
///
/// Cached overlays are keyed by anchor and tip hashes and pruned lazily after blocks leave the
/// context's live state.
#[derive(Default)]
pub(crate) struct OverlayManager {
    state_trie_overlays: OverlayCache<TrieInputSorted>,
    execution_overlays: OverlayCache<ExecutionOverlay>,
    /// The live in-memory state removal generation the overlay caches were last pruned at.
    pruned_generation: AtomicU64,
    changeset_cache: ChangesetCache,
    #[cfg(feature = "rayon")]
    worker_pool: Option<Arc<WorkerPool>>,
    metrics: StateTrieOverlayMetrics,
    execution_metrics: ExecutionOverlayMetrics,
    builder_metrics: OverlayBuilderMetrics,
    provider_metrics: OverlayStateProviderFactoryMetrics,
    proof_cursor_metrics: ProofCursorMetrics,
}

impl fmt::Debug for OverlayManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayManager")
            .field("state_trie_overlays", &self.state_trie_overlays.len())
            .field("execution_overlays", &self.execution_overlays.len())
            .finish_non_exhaustive()
    }
}

impl OverlayManager {
    /// Creates a manager that computes cached overlays using the given worker pool.
    #[cfg(feature = "rayon")]
    pub(super) fn with_worker_pool(worker_pool: Arc<WorkerPool>) -> Self {
        Self { worker_pool: Some(worker_pool), ..Self::default() }
    }

    #[cfg(feature = "rayon")]
    pub(super) fn worker_pool(&self) -> Option<&WorkerPool> {
        self.worker_pool.as_deref()
    }

    /// Collects the anchors of cached or in-progress execution overlays ending at `tip_hash`.
    #[cfg(feature = "rayon")]
    pub(super) fn cached_execution_overlay_anchors(&self, tip_hash: B256) -> Vec<B256> {
        self.execution_overlays
            .entries
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                (key.tip_hash == tip_hash).then_some(key.anchor_hash)
            })
            .collect()
    }

    pub(crate) const fn changeset_cache(&self) -> &ChangesetCache {
        &self.changeset_cache
    }

    /// Returns the metrics shared by overlay builders using this context.
    pub(crate) const fn builder_metrics(&self) -> &OverlayBuilderMetrics {
        &self.builder_metrics
    }

    /// Returns the metrics shared by overlay state providers using this context.
    pub(crate) const fn provider_metrics(&self) -> &OverlayStateProviderFactoryMetrics {
        &self.provider_metrics
    }

    /// Returns the proof cursor metrics shared by overlay state providers using this context.
    pub(crate) const fn proof_cursor_metrics(&self) -> &ProofCursorMetrics {
        &self.proof_cursor_metrics
    }

    /// Drops the cached overlays that can no longer be built from the in-memory state, if blocks
    /// were removed from it since this was done last.
    fn prune_overlays_if_blocks_removed<N: NodePrimitives>(&self, live_state: &InMemoryState<N>) {
        let generation = live_state.removal_generation();
        if self.pruned_generation.swap(generation, Ordering::Relaxed) != generation {
            self.prune_unreachable_overlays(live_state);
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
    fn prune_unreachable_overlays<N: NodePrimitives>(&self, live_state: &InMemoryState<N>) {
        let overlays_before = self.state_trie_overlays.len() + self.execution_overlays.len();
        self.state_trie_overlays
            .retain(|key, _| live_state_reaches(live_state, key.tip_hash, key.anchor_hash));
        self.execution_overlays
            .retain(|key, _| live_state_reaches(live_state, key.tip_hash, key.anchor_hash));
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

    /// Returns the flattened trie overlay from `anchor_hash` to `parent_state`.
    ///
    /// The parent's snapshot chain supplies the blocks. The context's live state is used only
    /// to admit and prune cache entries.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(tip_hash = %parent_state.hash(), anchor_hash = %anchor_hash)
    )]
    pub(super) fn overlay_for_parent<N: NodePrimitives>(
        &self,
        live_state: &InMemoryState<N>,
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
                live_state,
                (&self.state_trie_overlays, &self.metrics),
                anchor_hash,
                parent_state,
                cache_config,
                |input, span| {
                    compute_with_worker_pool(
                        input,
                        anchor_hash,
                        &self.metrics,
                        #[cfg(feature = "rayon")]
                        self.worker_pool().filter(|_| cache_config.write_to_cache),
                        span,
                        compute_state_trie_overlay,
                    )
                },
            )?
            .into_ready(&self.metrics);
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    /// Returns execution data from `anchor_hash` to `parent_state`.
    ///
    /// The parent's snapshot chain supplies the blocks. The context's live state is used only
    /// to admit and prune cache entries.
    #[tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(tip_hash = %parent_state.hash(), anchor_hash = %anchor_hash)
    )]
    pub(super) fn execution_overlay_for_block_state<N: NodePrimitives>(
        &self,
        live_state: &InMemoryState<N>,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
    ) -> Result<Arc<ExecutionOverlay>, OverlayError> {
        Ok(self
            .execution_overlay_for_parent_inner(
                live_state,
                parent_state,
                anchor_hash,
                cache_config,
            )?
            .into_ready(&self.execution_metrics))
    }

    #[cfg(feature = "rayon")]
    pub(super) fn precompute_execution_overlay_for_parent<N: NodePrimitives>(
        &self,
        live_state: &InMemoryState<N>,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(), OverlayError> {
        let parent_state = live_state
            .state_by_hash(parent_hash)
            .ok_or(OverlayError::UnreachableAnchor { tip_hash: parent_hash, anchor_hash })?;
        self.execution_overlay_for_parent_inner(
            live_state,
            &parent_state,
            anchor_hash,
            OverlayCacheConfig::default(),
        )
        .map(drop)
    }

    fn execution_overlay_for_parent_inner<N: NodePrimitives>(
        &self,
        live_state: &InMemoryState<N>,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
    ) -> Result<OverlayCacheEntry<ExecutionOverlay>, OverlayError> {
        let parent_hash = parent_state.hash();
        if parent_hash == anchor_hash {
            return Ok(OverlayCacheEntry::Ready(Arc::new(ExecutionOverlay::default())))
        }

        self.get_or_compute_overlay(
            live_state,
            (&self.execution_overlays, &self.execution_metrics),
            anchor_hash,
            parent_state,
            cache_config,
            |input, span| {
                compute_with_worker_pool(
                    input,
                    anchor_hash,
                    &self.execution_metrics,
                    #[cfg(feature = "rayon")]
                    self.worker_pool().filter(|_| cache_config.write_to_cache),
                    span,
                    compute_execution_overlay,
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
    fn get_or_compute_overlay<N: NodePrimitives, T, M>(
        &self,
        live_state: &InMemoryState<N>,
        (cache, metrics): (&OverlayCache<T>, &M),
        anchor_hash: B256,
        parent_state: &BlockState<N>,
        cache_config: OverlayCacheConfig,
        compute: impl FnOnce(ComputeOverlayInput<N, T>, tracing::Span) -> T,
    ) -> Result<OverlayCacheEntry<T>, OverlayError>
    where
        M: OverlayCacheMetrics,
    {
        self.prune_overlays_if_blocks_removed(live_state);

        let tip_hash = parent_state.hash();
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        let span = tracing::Span::current();
        if let Some(entry) = cache.entries.get(&key).map(|entry| entry.value().clone()) {
            metrics.record_cache_reuse();
            span.record("cache_reused", true);
            return Ok(entry)
        }
        span.record("cache_reused", false);

        // Resolve the block path before locking the child entry.
        let mut blocks = blocks_from_parent_state(parent_state, anchor_hash)?;
        span.record("block_count", blocks.len());

        // A parent state from an in-memory snapshot can reach blocks the live state no longer
        // tracks. Such an overlay would only be pruned at the next removal, so it is not cached.
        if !cache_config.write_to_cache || !live_state_reaches(live_state, tip_hash, anchor_hash) {
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
            return Ok(OverlayCacheEntry::Ready(Arc::new(compute(compute_input, span))))
        }

        let parent_hash = parent_state.parent_hash();
        cache.retain(|sibling_key, entry| {
            sibling_key.tip_hash == tip_hash ||
                !matches!(entry, OverlayCacheEntry::Ready(_)) ||
                live_state
                    .state_by_hash(sibling_key.tip_hash)
                    .is_none_or(|state| state.parent_hash() != parent_hash)
        });

        let waiter = match cache.entries.entry(key) {
            Entry::Occupied(entry) => {
                metrics.record_cache_reuse();
                span.record("cache_reused", true);
                return Ok(entry.get().clone())
            }
            Entry::Vacant(entry) => {
                metrics.record_cache_fill();
                let waiter = Arc::new(OverlayWaiter::new());
                entry.insert(OverlayCacheEntry::Computing(Arc::clone(&waiter)));
                waiter
            }
        };

        let parent_input = blocks.first().and_then(|block| {
            let parent_hash = block.recovered_block().parent_hash();
            (parent_hash != anchor_hash)
                .then(|| cache.take_ready(&OverlayCacheKey { anchor_hash, tip_hash: parent_hash }))
                .flatten()
        });
        span.record("parent_overlay_reused", parent_input.is_some());
        let compute_input = match parent_input {
            Some(parent_input) => {
                ComputeOverlayInput::ExtendCached { block: blocks.swap_remove(0), parent_input }
            }
            None => ComputeOverlayInput::MergeBlocks(blocks),
        };
        let input = Arc::new(compute(compute_input, span));
        waiter.finish(Arc::clone(&input));

        if let Entry::Occupied(mut entry) = cache.entries.entry(key) {
            // Only publish if this task's waiter was not pruned during the computation.
            if matches!(entry.get(), OverlayCacheEntry::Computing(existing) if Arc::ptr_eq(existing, &waiter))
            {
                entry.insert(OverlayCacheEntry::Ready(Arc::clone(&input)));
            }
        }

        Ok(OverlayCacheEntry::Ready(input))
    }
}

/// Controls whether to retain newly computed overlays in the manager cache.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OverlayCacheConfig {
    /// An overlay is retained only while the live state reaches its anchor from its tip.
    pub(crate) write_to_cache: bool,
}

impl Default for OverlayCacheConfig {
    fn default() -> Self {
        Self { write_to_cache: true }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct OverlayCacheKey {
    anchor_hash: B256,
    tip_hash: B256,
}

struct OverlayCache<T> {
    entries: DashMap<OverlayCacheKey, OverlayCacheEntry<T>>,
}

impl<T> Default for OverlayCache<T> {
    fn default() -> Self {
        Self { entries: Default::default() }
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

impl<T> OverlayCacheEntry<T> {
    /// Waits for an existing computation if the caller needs the value.
    fn into_ready(self, metrics: &impl OverlayCacheMetrics) -> Arc<T> {
        match self {
            Self::Ready(input) => input,
            Self::Computing(waiter) => waiter.wait(metrics),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::overlay_context_with_writer, OverlayContext};
    use alloy_primitives::{map::HashMap, Address, U256};
    use evm2::{bytecode::Bytecode, evm::AccountInfo};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock, NewCanonicalChain};
    use reth_ethereum_primitives::EthPrimitives;
    use reth_primitives_traits::Account;
    #[cfg(feature = "rayon")]
    use reth_tasks::WorkerPool;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage, SortedTrieData};
    use reth_trie_common::bundle_state::BundleState;
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
                AccountInfo { nonce: id as u64, balance: U256::from(id), ..Default::default() },
            )
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(id)))]))
            .contract(code_hash, Bytecode::new_raw(vec![id].into()))
            .build();
        let mut execution_output = (*block.execution_output).clone();
        execution_output.state = state;

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::new(execution_output),
            SortedTrieData::new(Arc::new(hashed_state), Arc::new(TrieUpdatesSorted::default())),
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
            in_memory_state: &InMemoryState<EthPrimitives>,
            parent_hash: B256,
            anchor_hash: B256,
        ) -> Result<Arc<ExecutionOverlay>, OverlayError> {
            if parent_hash == anchor_hash {
                return Ok(Arc::new(ExecutionOverlay::default()))
            }
            let parent_state = in_memory_state
                .state_by_hash(parent_hash)
                .ok_or(OverlayError::UnreachableAnchor { tip_hash: parent_hash, anchor_hash })?;
            self.execution_overlay_for_block_state(
                in_memory_state,
                &parent_state,
                anchor_hash,
                OverlayCacheConfig::default(),
            )
        }
    }

    fn overlay_for_parent(
        manager: &OverlayManager,
        in_memory_state: &InMemoryState<EthPrimitives>,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), OverlayError> {
        let parent_state = in_memory_state
            .state_by_hash(parent_hash)
            .ok_or(OverlayError::UnreachableAnchor { tip_hash: parent_hash, anchor_hash })?;
        manager.overlay_for_parent(
            in_memory_state,
            &parent_state,
            anchor_hash,
            OverlayCacheConfig::default(),
        )
    }

    #[reth_concurrency::test]
    fn builds_trie_overlay_for_inserted_blocks() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();

        let (_, state) = overlay_for_parent(
            manager,
            in_memory_state,
            blocks[2].recovered_block().hash(),
            anchor_hash,
        )
        .unwrap();
        assert_eq!(state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let (_, short) = overlay_for_parent(
            manager,
            in_memory_state,
            blocks[2].recovered_block().hash(),
            short_anchor,
        )
        .unwrap();
        assert_eq!(short.accounts.len(), 1);
        let (_, cached_short) = overlay_for_parent(
            manager,
            in_memory_state,
            blocks[2].recovered_block().hash(),
            short_anchor,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&short, &cached_short));
    }

    #[reth_concurrency::test]
    fn builds_execution_overlay_for_inserted_blocks() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let overlay = manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[2].recovered_block().hash(),
                anchor_hash,
            )
            .unwrap();

        for id in 1..=3 {
            let address = Address::with_last_byte(id);
            let code_hash = B256::with_last_byte(id + 64);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().nonce, id as u64);
            assert_eq!(overlay.storage()[&address][&U256::from(id)], U256::from(id));
            assert_eq!(overlay.code_hashes()[&code_hash], Bytecode::new_raw(vec![id].into()));
        }
        assert_eq!(
            overlay.block_hashes(),
            blocks[..=2].iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(),
        );

        let cached = manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[2].recovered_block().hash(),
                anchor_hash,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&overlay, &cached));

        let short_anchor = blocks[1].recovered_block().hash();
        let short = manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[2].recovered_block().hash(),
                short_anchor,
            )
            .unwrap();
        assert_eq!(short.accounts().len(), 1);
    }

    #[reth_concurrency::test]
    fn computing_sibling_evicts_ready_cached_overlays() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
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

        overlay_for_parent(manager, in_memory_state, sibling_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(in_memory_state, sibling_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&first_key));
        assert!(!manager.execution_overlays.entries.contains_key(&first_key));
    }

    #[reth_concurrency::test]
    fn promotes_ready_parent_overlays_to_the_child() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };

        overlay_for_parent(manager, in_memory_state, parent_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(in_memory_state, parent_hash, anchor_hash).unwrap();

        overlay_for_parent(manager, in_memory_state, child_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(in_memory_state, child_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(manager.state_trie_overlays.entries.contains_key(&child_key));
        assert!(!manager.execution_overlays.entries.contains_key(&parent_key));
        assert!(manager.execution_overlays.entries.contains_key(&child_key));
    }

    #[reth_concurrency::test]
    fn promotes_parent_overlays_held_by_callers() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        for block in &blocks {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };

        overlay_for_parent(manager, in_memory_state, parent_hash, anchor_hash).unwrap();
        let state_parent = manager
            .state_trie_overlays
            .entries
            .get(&parent_key)
            .and_then(|entry| match entry.value() {
                OverlayCacheEntry::Ready(input) => Some(Arc::clone(input)),
                OverlayCacheEntry::Computing(_) => None,
            })
            .unwrap();
        let execution_parent = manager
            .execution_overlay_for_parent(in_memory_state, parent_hash, anchor_hash)
            .unwrap();

        let (_, child_state) =
            overlay_for_parent(manager, in_memory_state, child_hash, anchor_hash).unwrap();
        let child_execution =
            manager.execution_overlay_for_parent(in_memory_state, child_hash, anchor_hash).unwrap();

        assert!(!manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(!manager.execution_overlays.entries.contains_key(&parent_key));
        assert_eq!(state_parent.state.accounts.len(), 2);
        assert_eq!(execution_parent.accounts().len(), 2);
        assert_eq!(child_state.accounts.len(), 3);
        assert_eq!(child_execution.accounts().len(), 3);
    }

    #[reth_concurrency::test]
    fn does_not_cache_or_take_parent_overlays_for_untracked_blocks() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        for block in &blocks[..2] {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let parent_hash = blocks[1].recovered_block().hash();
        let child_hash = blocks[2].recovered_block().hash();
        let parent_key = OverlayCacheKey { anchor_hash, tip_hash: parent_hash };
        let child_key = OverlayCacheKey { anchor_hash, tip_hash: child_hash };
        let parent_state = in_memory_state.state_by_hash(parent_hash).unwrap();
        let child_state = BlockState::with_parent(blocks[2].clone(), Some(parent_state));
        let cache_config = OverlayCacheConfig { write_to_cache: false };

        overlay_for_parent(manager, in_memory_state, parent_hash, anchor_hash).unwrap();
        manager.execution_overlay_for_parent(in_memory_state, parent_hash, anchor_hash).unwrap();

        let (_, state) = manager
            .overlay_for_parent(in_memory_state, &child_state, anchor_hash, cache_config)
            .unwrap();
        let execution = manager
            .execution_overlay_for_block_state(
                in_memory_state,
                &child_state,
                anchor_hash,
                cache_config,
            )
            .unwrap();

        assert_eq!(state.accounts.len(), 3);
        assert_eq!(execution.accounts().len(), 3);
        assert!(manager.state_trie_overlays.entries.contains_key(&parent_key));
        assert!(!manager.state_trie_overlays.entries.contains_key(&child_key));
        assert!(manager.execution_overlays.entries.contains_key(&parent_key));
        assert!(!manager.execution_overlays.entries.contains_key(&child_key));
    }

    #[cfg(feature = "rayon")]
    #[reth_concurrency::test]
    fn uncached_overlays_do_not_use_worker_pool() {
        let worker_pool = Arc::new(WorkerPool::new(1, "uncached-overlay-test"));
        let context =
            OverlayContext::with_worker_pool(InMemoryState::read_only(), Arc::clone(&worker_pool));
        let block = test_blocks().remove(0);
        let anchor_hash = block.recovered_block().parent_hash();
        let parent_state = BlockState::new(block);
        let cache_config = OverlayCacheConfig { write_to_cache: false };

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        worker_pool.spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();

        let (completed_tx, completed_rx) = mpsc::channel();
        let task = thread::spawn(move || {
            let manager = context.manager();
            let in_memory_state = context.in_memory_state();
            let execution = manager.execution_overlay_for_block_state(
                in_memory_state,
                &parent_state,
                anchor_hash,
                cache_config,
            );
            let state = manager.overlay_for_parent(
                in_memory_state,
                &parent_state,
                anchor_hash,
                cache_config,
            );
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
    #[reth_concurrency::test]
    fn precomputes_execution_overlay_for_cached_parent() {
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let context = OverlayContext::with_worker_pool(
            in_memory_state,
            Arc::new(WorkerPool::new(1, "execution-overlay-test")),
        );
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        let anchor_hash = blocks[0].recovered_block().parent_hash();

        in_memory_state_writer.insert_executed(blocks[0].clone());
        manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[0].recovered_block().hash(),
                anchor_hash,
            )
            .unwrap();

        in_memory_state_writer.insert_executed(blocks[1].clone());
        context.precompute_child_execution_overlays(blocks[1].recovered_block().hash());
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
    #[reth_concurrency::test]
    fn precomputes_execution_overlay_after_anchor_advances() {
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let context = OverlayContext::with_worker_pool(
            in_memory_state,
            Arc::new(WorkerPool::new(1, "execution-overlay-test")),
        );
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let tip_hash = blocks[2].recovered_block().hash();
        let old_anchor = blocks[0].recovered_block().parent_hash();
        manager.execution_overlay_for_parent(in_memory_state, tip_hash, old_anchor).unwrap();

        // The state/trie frontier advances to block 1. Block 2 may already be persisted at the
        // Finish frontier, but its execution state must remain in the overlay until trie
        // persistence.
        let new_anchor = blocks[0].recovered_block().hash();
        in_memory_state_writer.on_blocks_persisted(
            blocks[1].recovered_block().num_hash(),
            blocks[0].recovered_block().num_hash(),
            None,
        );

        context.precompute_execution_overlay(tip_hash, new_anchor);

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
        // The overlay request of the precompute dropped the overlay anchored at the removed
        // block's parent.
        assert!(!manager
            .execution_overlays
            .entries
            .contains_key(&OverlayCacheKey { anchor_hash: old_anchor, tip_hash }));
        for id in 2..=3 {
            let address = Address::with_last_byte(id);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().nonce, id as u64);
            assert_eq!(overlay.storage()[&address][&U256::from(id)], U256::from(id));
        }
        let cached =
            manager.execution_overlay_for_parent(in_memory_state, tip_hash, new_anchor).unwrap();
        assert!(Arc::ptr_eq(&overlay, &cached));
    }

    #[cfg(feature = "rayon")]
    #[reth_concurrency::test]
    fn execution_overlay_precompute_does_not_wait_for_pending_entry() {
        let worker_pool = Arc::new(WorkerPool::new(1, "execution-overlay-pending-test"));
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let context = OverlayContext::with_worker_pool(in_memory_state, Arc::clone(&worker_pool));
        let manager = context.manager();
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
            context
                .manager()
                .precompute_execution_overlay_for_parent(
                    context.in_memory_state(),
                    tip_hash,
                    anchor_hash,
                )
                .unwrap();
            tx.send(()).unwrap();
        });

        let completed = rx.recv_timeout(Duration::from_millis(100));
        waiter.finish(Arc::new(ExecutionOverlay::default()));
        assert!(completed.is_ok(), "execution overlay precompute waited for pending entry");
    }

    #[reth_concurrency::test]
    fn required_lookup_waits_for_in_progress_overlay() {
        let context = OverlayContext::default();
        let manager = context.manager();
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
            let res = context
                .manager()
                .overlay_for_parent(
                    context.in_memory_state(),
                    &parent_state,
                    key.anchor_hash,
                    OverlayCacheConfig::default(),
                )
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

    #[reth_concurrency::test]
    fn does_not_cache_overlays_the_live_state_no_longer_reaches() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let anchor_hash = blocks[0].recovered_block().parent_hash();
        let view = context.snapshot_during(|| ());
        let tip = view.snapshot().state_by_hash(blocks[2].recovered_block().hash()).unwrap();

        // Persistence removes the oldest block, so the live chain of the tip no longer reaches
        // the anchor the snapshot's chain still builds on.
        let persisted = blocks[0].recovered_block().num_hash();
        in_memory_state_writer.on_blocks_persisted(persisted, persisted, None);

        let (_, state) = manager
            .overlay_for_parent(in_memory_state, &tip, anchor_hash, OverlayCacheConfig::default())
            .unwrap();
        assert_eq!(state.accounts.len(), 3);
        assert_eq!(manager.state_trie_overlays.len(), 0);
    }

    #[reth_concurrency::test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let manager = context.manager();
        let in_memory_state = context.in_memory_state();
        let blocks = test_blocks();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        let original_anchor = blocks[0].recovered_block().parent_hash();
        overlay_for_parent(
            manager,
            in_memory_state,
            blocks[2].recovered_block().hash(),
            original_anchor,
        )
        .unwrap();
        manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[2].recovered_block().hash(),
                original_anchor,
            )
            .unwrap();

        let tip_hash = blocks[2].recovered_block().hash();
        let original_key = OverlayCacheKey { anchor_hash: original_anchor, tip_hash };
        assert!(manager.state_trie_overlays.entries.contains_key(&original_key));
        assert!(manager.execution_overlays.entries.contains_key(&original_key));

        // Removing blocks from the in-memory state is all it takes: the next overlay request
        // drops the overlays anchored below the removed blocks instead of serving them.
        let anchor_hash = blocks[1].recovered_block().hash();
        let persisted = blocks[1].recovered_block().num_hash();
        in_memory_state_writer.on_blocks_persisted(persisted, persisted, None);

        let expected = OverlayError::UnreachableAnchor { tip_hash, anchor_hash: original_anchor };
        assert_eq!(
            overlay_for_parent(manager, in_memory_state, tip_hash, original_anchor).unwrap_err(),
            expected,
        );
        assert!(!manager.state_trie_overlays.entries.contains_key(&original_key));
        assert!(!manager.execution_overlays.entries.contains_key(&original_key));
        assert_eq!(
            manager
                .execution_overlay_for_parent(in_memory_state, tip_hash, original_anchor)
                .unwrap_err(),
            expected,
        );

        let (_, state) = overlay_for_parent(
            manager,
            in_memory_state,
            blocks[2].recovered_block().hash(),
            anchor_hash,
        )
        .unwrap();
        assert_eq!(state.accounts.len(), 1);
        let execution = manager
            .execution_overlay_for_parent(
                in_memory_state,
                blocks[2].recovered_block().hash(),
                anchor_hash,
            )
            .unwrap();
        assert_eq!(execution.accounts().len(), 1);
    }
}
