//! Binds the overlay caches to the in-memory state and its snapshots.

use crate::{ExecutionOverlay, OverlayError};
use alloy_primitives::{BlockNumber, B256};
use manager::OverlayManager;
use parking_lot::Mutex;
use reth_chain_state::{
    BlockState, InMemoryState, InMemoryStateSnapshot, PreservedSparseTrie, Snapshotted,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{FastInstant, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted};
use std::{fmt, ops::Deref, sync::Arc, time::Duration};
use tracing::debug;

#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;

mod manager;
pub(crate) use manager::OverlayCacheConfig;

/// The live in-memory state and the overlay caches built for it.
///
/// Clones share one state, manager and preserved sparse trie. Use [`Self::snapshot_during`] to
/// pair a database transaction with the in-memory blocks tracked while it was opened.
#[derive(Clone)]
pub struct OverlayContext<N: NodePrimitives = EthPrimitives> {
    inner: Arc<OverlayContextInner<N>>,
}

struct OverlayContextInner<N: NodePrimitives> {
    in_memory_state: InMemoryState<N>,
    manager: OverlayManager,
    preserved_sparse_trie: Mutex<Option<PreservedSparseTrie>>,
}

impl<N: NodePrimitives> OverlayContext<N> {
    /// Creates a context that computes cached overlays on the calling thread.
    pub fn new(in_memory_state: InMemoryState<N>) -> Self {
        Self {
            inner: Arc::new(OverlayContextInner {
                in_memory_state,
                manager: OverlayManager::default(),
                preserved_sparse_trie: Mutex::default(),
            }),
        }
    }

    /// Creates a context that computes cached overlays using the given worker pool.
    #[cfg(feature = "rayon")]
    pub fn with_worker_pool(
        in_memory_state: InMemoryState<N>,
        worker_pool: Arc<WorkerPool>,
    ) -> Self {
        Self {
            inner: Arc::new(OverlayContextInner {
                in_memory_state,
                manager: OverlayManager::with_worker_pool(worker_pool),
                preserved_sparse_trie: Mutex::default(),
            }),
        }
    }

    /// Returns the live in-memory state this context's caches are built for.
    pub fn in_memory_state(&self) -> &InMemoryState<N> {
        &self.inner.in_memory_state
    }

    pub(crate) fn manager(&self) -> &OverlayManager {
        &self.inner.manager
    }

    /// Runs `f` and pairs its result with the blocks tracked while it ran.
    ///
    /// As with [`InMemoryState::snapshot_during`], `f` may be retried and must not access
    /// this context's in-memory state: the final attempt runs with that state's read lock held.
    pub fn snapshot_during<R>(&self, f: impl FnMut() -> R) -> OverlayView<R, N> {
        OverlayView {
            context: self.clone(),
            snapshotted: self.in_memory_state().snapshot_during(f),
        }
    }

    /// Evicts cached changesets for blocks below `up_to_block`.
    pub fn evict_cached_changesets(&self, up_to_block: BlockNumber) {
        self.manager().changeset_cache().evict(up_to_block);
    }

    /// Takes the preserved sparse trie, if present.
    pub fn take_sparse_trie(&self) -> Option<PreservedSparseTrie> {
        self.inner.preserved_sparse_trie.lock().take()
    }

    /// Stores a preserved sparse trie for later reuse.
    pub fn store_sparse_trie(&self, trie: PreservedSparseTrie) {
        *self.inner.preserved_sparse_trie.lock() = Some(trie);
    }

    /// Clears any preserved sparse trie state.
    pub fn clear_sparse_trie(&self) {
        *self.inner.preserved_sparse_trie.lock() = None;
    }

    /// Acquires and releases the preserved sparse trie lock and returns the time spent waiting.
    pub fn wait_for_sparse_trie_availability(&self) -> Duration {
        let start = FastInstant::now();
        let _guard = self.inner.preserved_sparse_trie.lock();
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

    /// Optimistically extends cached execution overlays ending at the parent of `block_hash`.
    ///
    /// This returns without waiting. Nothing is scheduled if the block is not tracked, if no
    /// overlay ending at its parent is cached, or without a worker pool.
    #[cfg_attr(feature = "rayon", tracing::instrument(
        level = "trace",
        target = "storage::overlay::manager",
        skip_all,
        fields(block_hash = %block_hash, parent_hash = tracing::field::Empty)
    ))]
    pub fn precompute_child_execution_overlays(&self, block_hash: B256) {
        #[cfg(feature = "rayon")]
        {
            if self.manager().worker_pool().is_none() {
                return
            }

            let Some(state) = self.in_memory_state().state_by_hash(block_hash) else { return };
            let parent_hash = state.parent_hash();
            tracing::Span::current().record("parent_hash", tracing::field::display(parent_hash));

            let cached_parent_overlays =
                self.manager().cached_execution_overlay_anchors(parent_hash);
            debug!(
                target: "storage::overlay::manager",
                %block_hash,
                %parent_hash,
                cached_parent_overlays = cached_parent_overlays.len(),
                "precomputing execution overlays for the child of cached overlays"
            );

            for anchor_hash in cached_parent_overlays {
                self.precompute_execution_overlay(block_hash, anchor_hash);
            }
        }

        #[cfg(not(feature = "rayon"))]
        let _ = block_hash;
    }

    /// Optimistically computes an execution overlay from `anchor_hash` to `tip_hash`.
    ///
    /// This returns without waiting. Nothing is scheduled without a worker pool or when the
    /// tip is already at the anchor. The task skips work that a concurrent persistence or reorg
    /// made unavailable, or that another task is already computing.
    #[cfg(feature = "rayon")]
    pub fn precompute_execution_overlay(&self, tip_hash: B256, anchor_hash: B256) {
        if tip_hash == anchor_hash {
            return
        }
        let Some(worker_pool) = self.manager().worker_pool() else { return };
        let context = self.clone();
        let parent_span = tracing::Span::current();
        worker_pool.spawn(move || {
            let _parent = parent_span.enter();
            let _span = tracing::trace_span!(
                target: "storage::overlay::manager",
                "precompute_execution_overlay",
                %tip_hash,
                %anchor_hash,
            )
            .entered();
            if let Err(err) = context.precompute_execution_overlay_for_parent(tip_hash, anchor_hash) {
                debug!(target: "storage::overlay::manager", %err, "Skipping execution overlay precompute");
            }
        });
    }
    /// Resolves the trie overlay for a snapshot's parent using this context's caches.
    pub(crate) fn overlay_for_parent(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), OverlayError> {
        self.manager().overlay_for_parent(
            self.in_memory_state(),
            parent_state,
            anchor_hash,
            cache_config,
        )
    }

    /// Resolves the execution overlay for a snapshot's parent using this context's caches.
    pub(crate) fn execution_overlay_for_block_state(
        &self,
        parent_state: &BlockState<N>,
        anchor_hash: B256,
        cache_config: OverlayCacheConfig,
    ) -> Result<Arc<ExecutionOverlay>, OverlayError> {
        self.manager().execution_overlay_for_block_state(
            self.in_memory_state(),
            parent_state,
            anchor_hash,
            cache_config,
        )
    }

    #[cfg(feature = "rayon")]
    pub(crate) fn precompute_execution_overlay_for_parent(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<(), OverlayError> {
        self.manager().precompute_execution_overlay_for_parent(
            self.in_memory_state(),
            tip_hash,
            anchor_hash,
        )
    }
}

impl<N: NodePrimitives> fmt::Debug for OverlayContext<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayContext").field("manager", self.manager()).finish_non_exhaustive()
    }
}

impl<N: NodePrimitives> Default for OverlayContext<N> {
    /// Creates a context for a database with no in-memory blocks.
    fn default() -> Self {
        Self::new(InMemoryState::read_only())
    }
}

/// A value and the in-memory blocks tracked while it was produced, bound to their context.
///
/// Create this with [`OverlayContext::snapshot_during`] so the value, snapshot, and live state
/// used to maintain the overlay caches cannot come from different contexts.
#[derive(Debug)]
pub struct OverlayView<T, N: NodePrimitives = EthPrimitives> {
    context: OverlayContext<N>,
    snapshotted: Snapshotted<T, N>,
}

impl<T, N: NodePrimitives> OverlayView<T, N> {
    /// Returns the in-memory blocks tracked while the value was produced.
    pub const fn snapshot(&self) -> &InMemoryStateSnapshot<N> {
        self.snapshotted.snapshot()
    }

    pub(crate) const fn context(&self) -> &OverlayContext<N> {
        &self.context
    }

    pub(crate) fn into_parts(self) -> (T, InMemoryStateSnapshot<N>, OverlayContext<N>) {
        let (value, snapshot) = self.snapshotted.into_parts();
        (value, snapshot, self.context)
    }
}

impl<T, E, N: NodePrimitives> OverlayView<Result<T, E>, N> {
    /// Returns the bound view of a successful result, or its error.
    pub fn transpose(self) -> Result<OverlayView<T, N>, E> {
        Ok(OverlayView { context: self.context, snapshotted: self.snapshotted.transpose()? })
    }
}

impl<T, N: NodePrimitives> Deref for OverlayView<T, N> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.snapshotted
    }
}
