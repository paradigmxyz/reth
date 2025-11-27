use alloy_primitives::B256;
use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted, TrieInputSorted,
};
use std::{
    fmt,
    sync::{Arc, LazyLock, OnceLock},
};

/// Metrics for deferred trie computation.
#[derive(Metrics)]
#[metrics(scope = "sync.block_validation")]
struct DeferredTrieMetrics {
    /// Number of times deferred trie data required synchronous computation (fallback path).
    deferred_trie_sync_fallback: Counter,
}

/// Shared handle to asynchronously populated trie data.
///
/// Uses `OnceLock::get_or_init` for thread-safe lazy initialization:
/// - First caller computes the result, others wait and receive the cached value
/// - Exactly one computation runs per handle (automatic deduplication)
///
/// Deadlock safety requires that ancestors form a DAG (no cycles or self-references).
#[derive(Clone)]
pub struct DeferredTrieData {
    inner: Arc<DeferredTrieInner>,
}

/// Internal state for deferred trie data.
struct DeferredTrieInner {
    /// Inputs for deferred computation (None for ready-constructed handles).
    inputs: Option<PendingInputs>,
    /// Cached computation result.
    computed: OnceLock<ComputedTrieData>,
}

static DEFERRED_TRIE_METRICS: LazyLock<DeferredTrieMetrics> =
    LazyLock::new(DeferredTrieMetrics::default);

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.inner.computed.get().is_some() { "ready" } else { "pending" };
        f.debug_struct("DeferredTrieData").field("state", &state).finish()
    }
}

impl DeferredTrieData {
    /// Create a new pending handle with inputs for deferred computation.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state from execution
    /// * `trie_updates` - Unsorted trie updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestor_overlay` - Pre-computed overlay from ancestor blocks (None if no in-memory
    ///   ancestors)
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        ancestor_overlay: Option<Arc<TrieInputSorted>>,
    ) -> Self {
        Self {
            inner: Arc::new(DeferredTrieInner {
                inputs: Some(PendingInputs {
                    hashed_state,
                    trie_updates,
                    anchor_hash,
                    ancestor_overlay,
                }),
                computed: OnceLock::new(),
            }),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately.
    /// [`Self::wait_cloned`] will return without any computation.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        let computed = OnceLock::new();
        let _ = computed.set(bundle);
        Self { inner: Arc::new(DeferredTrieInner { inputs: None, computed }) }
    }

    /// Returns trie data, computing synchronously if not already cached.
    ///
    /// Uses `OnceLock::get_or_init` for thread-safe lazy initialization:
    /// - If already computed: returns cached result immediately
    /// - If not computed: first caller computes, others wait for that result
    ///
    /// This guarantees exactly one computation per handle (automatic deduplication).
    #[tracing::instrument(level = "debug", target = "engine::tree", name = "wait_cloned", skip_all)]
    pub fn wait_cloned(&self) -> ComputedTrieData {
        self.inner
            .computed
            .get_or_init(|| {
                DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);
                self.compute_trie_data()
            })
            .clone()
    }

    /// Compute trie data synchronously from the stored inputs.
    ///
    /// This performs the same computation as the async task:
    /// 1. Sort the current block's hashed state and trie updates
    /// 2. Start with pre-computed ancestor overlay (or empty if none)
    /// 3. Extend the overlay with the current block's sorted data
    /// 4. Return the completed `ComputedTrieData`
    #[tracing::instrument(
        level = "debug",
        target = "engine::tree",
        name = "compute_trie_data",
        skip_all
    )]
    fn compute_trie_data(&self) -> ComputedTrieData {
        let inputs = self.inner.inputs.as_ref().expect("compute_trie_data called on ready handle");

        // Sort the current block's hashed state and trie updates
        let sorted_hashed_state = Arc::new(inputs.hashed_state.as_ref().clone().into_sorted());
        let sorted_trie_updates = Arc::new(inputs.trie_updates.as_ref().clone().into_sorted());

        // Start with ancestor overlay or empty (no recursive references!)
        let mut overlay =
            inputs.ancestor_overlay.as_ref().map(|o| (**o).clone()).unwrap_or_default();

        // Extend overlay with current block's sorted data
        {
            let state_mut = Arc::make_mut(&mut overlay.state);
            state_mut.extend_ref(sorted_hashed_state.as_ref());
        }
        {
            let nodes_mut = Arc::make_mut(&mut overlay.nodes);
            nodes_mut.extend_ref(sorted_trie_updates.as_ref());
        }

        ComputedTrieData::with_trie_input(
            sorted_hashed_state,
            sorted_trie_updates,
            inputs.anchor_hash,
            Arc::new(overlay),
        )
    }
}

/// Sorted trie data computed for an executed block.
/// These represent the complete set of sorted trie data required to persist
/// block state for, and generate proofs on top of a block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// Trie input bundled with its anchor hash, if available.
    pub anchored_trie_input: Option<AnchoredTrieInput>,
}

impl ComputedTrieData {
    /// Construct a bundle that includes trie input anchored to a persisted ancestor.
    pub const fn with_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        anchor_hash: B256,
        trie_input: Arc<TrieInputSorted>,
    ) -> Self {
        Self {
            hashed_state,
            trie_updates,
            anchored_trie_input: Some(AnchoredTrieInput { anchor_hash, trie_input }),
        }
    }

    /// Construct a bundle without trie input or anchor information.
    pub const fn without_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates, anchored_trie_input: None }
    }

    /// Returns the anchor hash, if present.
    pub fn anchor_hash(&self) -> Option<B256> {
        self.anchored_trie_input.as_ref().map(|anchored| anchored.anchor_hash)
    }

    /// Returns the trie input, if present.
    pub fn trie_input(&self) -> Option<&Arc<TrieInputSorted>> {
        self.anchored_trie_input.as_ref().map(|anchored| &anchored.trie_input)
    }
}

/// Inputs kept while a deferred trie computation is pending.
#[derive(Clone, Debug)]
struct PendingInputs {
    /// Unsorted hashed post-state from execution.
    hashed_state: Arc<HashedPostState>,
    /// Unsorted trie updates from state root computation.
    trie_updates: Arc<TrieUpdates>,
    /// The persisted ancestor hash this trie input is anchored to.
    anchor_hash: B256,
    /// Parent's cumulative trie state overlay. None if parent is persisted in DB.
    /// This is used to efficiently build the current block's overlay.
    ///
    /// Example:
    /// ```text
    /// [DB: A] -> [Mem: B] -> [Mem: C] -> [Mem: D (current)]
    ///
    /// C's trie_input = B + C (already merged)
    /// D's trie_input = ancestor_overlay + D = (B + C) + D
    /// ```
    ancestor_overlay: Option<Arc<TrieInputSorted>>,
}

/// Trie input bundled with its anchor hash.
///
/// This is used to store the trie input and anchor hash for a block together.
#[derive(Clone, Debug)]
pub struct AnchoredTrieInput {
    /// The persisted ancestor hash this trie input is anchored to.
    pub anchor_hash: B256,
    /// Trie input constructed from in-memory overlays.
    pub trie_input: Arc<TrieInputSorted>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::B256Map, U256};
    use reth_primitives_traits::Account;
    use reth_trie::updates::TrieUpdates;
    use std::{
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    fn empty_bundle() -> ComputedTrieData {
        ComputedTrieData {
            hashed_state: Arc::default(),
            trie_updates: Arc::default(),
            anchored_trie_input: None,
        }
    }

    fn empty_pending() -> DeferredTrieData {
        empty_pending_with_anchor(B256::ZERO)
    }

    fn empty_pending_with_anchor(anchor: B256) -> DeferredTrieData {
        DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            anchor,
            None, // No ancestor overlay
        )
    }

    /// Verifies that a ready handle returns immediately without computation.
    #[test]
    fn ready_returns_immediately() {
        let bundle = empty_bundle();
        let deferred = DeferredTrieData::ready(bundle.clone());

        let start = Instant::now();
        let result = deferred.wait_cloned();
        let elapsed = start.elapsed();

        assert_eq!(result.hashed_state, bundle.hashed_state);
        assert_eq!(result.trie_updates, bundle.trie_updates);
        assert_eq!(result.anchor_hash(), bundle.anchor_hash());
        assert!(elapsed < Duration::from_millis(20));
    }

    /// Verifies that a pending handle computes trie data synchronously via fallback.
    #[test]
    fn pending_computes_fallback() {
        let deferred = empty_pending();

        // wait_cloned should compute from inputs without blocking
        let start = Instant::now();
        let result = deferred.wait_cloned();
        let elapsed = start.elapsed();

        // Should return quickly (fallback computation)
        assert!(elapsed < Duration::from_millis(100));
        assert!(result.hashed_state.is_empty());
    }

    /// Verifies that fallback computation result is cached for subsequent calls.
    #[test]
    fn fallback_result_is_cached() {
        let deferred = empty_pending();

        // First call computes and should stash the result
        let first = deferred.wait_cloned();
        // Second call should reuse the cached result (same Arc pointer)
        let second = deferred.wait_cloned();

        assert!(Arc::ptr_eq(&first.hashed_state, &second.hashed_state));
        assert!(Arc::ptr_eq(&first.trie_updates, &second.trie_updates));
        assert_eq!(first.anchor_hash(), second.anchor_hash());
    }

    /// Verifies concurrent `wait_cloned` calls: one computes, others wait and get cached result.
    #[test]
    fn concurrent_wait_cloned_shares_result() {
        let deferred = empty_pending_with_anchor(B256::with_last_byte(100));

        // Spawn multiple threads that all call wait_cloned concurrently
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let d = deferred.clone();
                thread::spawn(move || d.wait_cloned())
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All results should have the same anchor (computed from inputs)
        for result in &results {
            assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(100)));
        }

        // All results should share the same Arc (same cached computation)
        for i in 1..results.len() {
            assert!(Arc::ptr_eq(&results[0].hashed_state, &results[i].hashed_state));
        }
    }

    /// Tests that ancestor overlay is merged during fallback computation and that the
    /// resulting `ComputedTrieData` uses the current block's anchor hash.
    #[test]
    fn ancestor_overlay_is_merged() {
        // Create an ancestor overlay with some data
        let ancestor_overlay = Arc::new(TrieInputSorted::default());

        // Create pending with ancestor overlay
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(2),
            Some(ancestor_overlay),
        );

        let result = deferred.wait_cloned();
        // Should have the current block's anchor
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(2)));
    }

    /// Ensures ancestor overlay state is extended with current block's state.
    #[test]
    fn ancestor_overlay_extended_with_current_block() {
        let key = B256::with_last_byte(1);

        // Ancestor overlay has nonce 1
        let ancestor_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );
        let mut ancestor_overlay = TrieInputSorted::default();
        Arc::make_mut(&mut ancestor_overlay.state).extend_ref(&ancestor_state);

        // Current block sets nonce to 2 (should override ancestor)
        let current_state = HashedPostState::default().with_accounts([(
            key,
            Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }),
        )]);

        let deferred = DeferredTrieData::pending(
            Arc::new(current_state),
            Arc::new(TrieUpdates::default()),
            B256::ZERO,
            Some(Arc::new(ancestor_overlay)),
        );

        let result = deferred.wait_cloned();
        let overlay_state = &result.anchored_trie_input.as_ref().unwrap().trie_input.state.accounts;
        assert_eq!(overlay_state.len(), 1);
        let (_, account) = &overlay_state[0];
        // Current block's nonce should take precedence
        assert_eq!(account.unwrap().nonce, 2);
    }

    /// Verifies that blocks without ancestor overlay work correctly.
    #[test]
    fn no_ancestor_overlay() {
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(1),
            None, // No ancestor overlay (first block after persisted state)
        );

        let result = deferred.wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(1)));
        assert!(result.hashed_state.is_empty());
    }

    /// Verifies concurrent computation with shared ancestor overlay.
    #[test]
    fn concurrent_with_shared_overlay() {
        let shared_overlay = Arc::new(TrieInputSorted::default());

        // Create multiple deferred handles sharing the same ancestor overlay
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let deferred = DeferredTrieData::pending(
                    Arc::new(HashedPostState::default()),
                    Arc::new(TrieUpdates::default()),
                    B256::with_last_byte(i),
                    Some(Arc::clone(&shared_overlay)),
                );
                thread::spawn(move || deferred.wait_cloned())
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each result should have its own anchor hash
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(i as u8)));
        }
    }

    /// Verifies that two independent blocks can be computed in parallel.
    #[test]
    fn parallel_independent_blocks() {
        let block1 = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(1),
            None,
        );
        let block2 = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(2),
            None,
        );

        let h1 = thread::spawn(move || block1.wait_cloned());
        let h2 = thread::spawn(move || block2.wait_cloned());

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        assert_eq!(r1.anchor_hash(), Some(B256::with_last_byte(1)));
        assert_eq!(r2.anchor_hash(), Some(B256::with_last_byte(2)));
    }
}
