use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted, TrieInputSorted,
};
use std::{
    fmt,
    sync::{Arc, LazyLock},
};

/// Metrics for deferred trie computation.
#[derive(Metrics)]
#[metrics(scope = "sync.block_validation")]
struct DeferredTrieMetrics {
    /// Number of times deferred trie data was ready (async task completed first).
    deferred_trie_async_ready: Counter,
    /// Number of times deferred trie data required synchronous computation (fallback path).
    deferred_trie_sync_fallback: Counter,
}

/// Shared handle to asynchronously populated trie data.
///
/// Uses a try-lock + blocking fallback approach:
/// - First attempts a non-blocking `try_lock` to check/compute the result
/// - If contended (another thread is computing), blocks until that thread finishes
///
/// Deadlock safety requires that ancestors form a DAG (no cycles or self-references).
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared deferred state holding either raw inputs (pending) or computed result (ready).
    state: Arc<Mutex<DeferredState>>,
}

static DEFERRED_TRIE_METRICS: LazyLock<DeferredTrieMetrics> =
    LazyLock::new(DeferredTrieMetrics::default);

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        match &*state {
            DeferredState::Pending(_) => {
                f.debug_struct("DeferredTrieData").field("state", &"pending").finish()
            }
            DeferredState::Ready(_) => {
                f.debug_struct("DeferredTrieData").field("state", &"ready").finish()
            }
        }
    }
}

impl DeferredTrieData {
    /// Create a new pending handle with inputs for deferred computation.
    ///
    /// # Safety Invariant
    /// The `ancestors` list must form a DAG (directed acyclic graph). Self-references
    /// or cycles will cause deadlock when `wait_cloned` is called.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state from execution
    /// * `trie_updates` - Unsorted trie updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestors` - Deferred trie data from ancestor blocks (must form a DAG)
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        ancestors: Vec<Self>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(DeferredState::Pending(PendingInputs {
                hashed_state,
                trie_updates,
                anchor_hash,
                ancestors,
            }))),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately.
    /// [`Self::wait_cloned`] will return without any computation.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { state: Arc::new(Mutex::new(DeferredState::Ready(bundle))) }
    }

    /// Returns trie data, computing synchronously if not already cached.
    ///
    /// Uses a try-lock + blocking fallback approach:
    /// - If `try_lock` succeeds: returns cached result or computes and caches
    /// - If `try_lock` fails (contended): blocks until the computing thread finishes
    ///
    /// This avoids duplicate computation when multiple threads access the same handle.
    pub fn wait_cloned(&self) -> ComputedTrieData {
        // Try to get the lock
        if let Some(mut state) = self.state.try_lock() {
            match &*state {
                // The async task has completed, return the cached result.
                DeferredState::Ready(bundle) => {
                    DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
                    return bundle.clone();
                }
                // The async task is still pending, compute the trie data synchronously from the
                // stored inputs.
                DeferredState::Pending(inputs) => {
                    DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);
                    let computed = Self::compute_from_inputs(inputs);
                    *state = DeferredState::Ready(computed.clone());
                    return computed;
                }
            }
        }

        // Lock is contended - another thread is computing. Block until released.
        // After acquiring, either return the cached result or compute and cache.
        //
        // Deadlock is avoided as long as the provided ancestors form a true ancestor chain (a DAG):
        // - Each block only waits on its ancestors (blocks on the path to the persisted root)
        // - Sibling blocks (forks) are never in each other's ancestor lists
        // - A block never waits on its descendants
        // Given that invariant, circular wait dependencies are impossible. Supplying a cycle in the
        // ancestor list would violate this assumption and could deadlock.
        let mut state = self.state.lock();
        match &*state {
            // The async task has completed, return the cached result.
            DeferredState::Ready(bundle) => {
                DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
                bundle.clone()
            }
            // The async task is still pending, compute the trie data synchronously from the stored
            // inputs.
            DeferredState::Pending(inputs) => {
                DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);
                let computed = Self::compute_from_inputs(inputs);
                *state = DeferredState::Ready(computed.clone());
                computed
            }
        }
    }

    /// Compute trie data synchronously from the stored inputs.
    ///
    /// This performs the same computation as the async task:
    /// 1. Sort the current block's hashed state and trie updates
    /// 2. Merge trie data from ancestor blocks
    /// 3. Extend the overlay with the current block's sorted data
    /// 4. Return the completed `ComputedTrieData`
    fn compute_from_inputs(inputs: &PendingInputs) -> ComputedTrieData {
        // Sort the current block's hashed state and trie updates
        let sorted_hashed_state = Arc::new(inputs.hashed_state.as_ref().clone().into_sorted());
        let sorted_trie_updates = Arc::new(inputs.trie_updates.as_ref().clone().into_sorted());

        // Merge trie data from ancestors (oldest -> newest so later state takes precedence)
        let mut overlay = TrieInputSorted::default();
        for ancestor in &inputs.ancestors {
            let ancestor_data = ancestor.wait_cloned();
            {
                let state_mut = Arc::make_mut(&mut overlay.state);
                state_mut.extend_ref(ancestor_data.hashed_state.as_ref());
            }
            {
                let nodes_mut = Arc::make_mut(&mut overlay.nodes);
                nodes_mut.extend_ref(ancestor_data.trie_updates.as_ref());
            }
        }

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

/// Internal state for deferred trie data.
enum DeferredState {
    /// Data is not yet available; raw inputs stored for fallback computation.
    Pending(PendingInputs),
    /// Data has been computed and is ready.
    Ready(ComputedTrieData),
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
    /// Deferred trie data from ancestor blocks for merging.
    ancestors: Vec<DeferredTrieData>,
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
            Vec::new(),
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

    /// Verifies concurrent wait_cloned calls: one computes, others wait and get cached result.
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

    /// Tests that ancestor trie data is merged during fallback computation and that the
    /// resulting `ComputedTrieData` uses the current block's anchor hash, not the ancestor's.
    #[test]
    fn ancestors_are_merged() {
        // Create ancestor with some data
        let ancestor_bundle = ComputedTrieData {
            hashed_state: Arc::default(),
            trie_updates: Arc::default(),
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(1),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
        };
        let ancestor = DeferredTrieData::ready(ancestor_bundle);

        // Create pending with ancestor
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(2),
            vec![ancestor],
        );

        let result = deferred.wait_cloned();
        // Should have the current block's anchor, not the ancestor's
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(2)));
    }

    /// Ensures ancestor overlays are merged oldest -> newest so latest state wins (no overwrite by
    /// older ancestors).
    #[test]
    fn ancestors_merge_in_chronological_order() {
        let key = B256::with_last_byte(1);
        // Oldest ancestor sets nonce to 1
        let oldest_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );
        // Newest ancestor overwrites nonce to 2
        let newest_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );

        let oldest = ComputedTrieData {
            hashed_state: Arc::new(oldest_state),
            trie_updates: Arc::default(),
            anchored_trie_input: None,
        };
        let newest = ComputedTrieData {
            hashed_state: Arc::new(newest_state),
            trie_updates: Arc::default(),
            anchored_trie_input: None,
        };

        // Pass ancestors oldest -> newest; newest should take precedence
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::ZERO,
            vec![DeferredTrieData::ready(oldest), DeferredTrieData::ready(newest)],
        );

        let result = deferred.wait_cloned();
        let overlay_state = &result.anchored_trie_input.as_ref().unwrap().trie_input.state.accounts;
        assert_eq!(overlay_state.len(), 1);
        let (_, account) = &overlay_state[0];
        assert_eq!(account.unwrap().nonce, 2);
    }

    // ==================== Deadlock Safety Tests ====================
    //
    // These tests verify that the implementation handles various ancestor chain
    // patterns without deadlock. The key invariant is that ancestors must form
    // a DAG (directed acyclic graph) - no cycles or self-references.
    //
    // IMPORTANT: Self-reference or cycles in ancestors WILL deadlock because
    // `compute_from_inputs` holds the mutex while calling `wait_cloned` on
    // ancestors. If an ancestor is the same object (or part of a cycle),
    // the thread will block waiting for a lock it already holds.

    /// Helper to create a chain of pending blocks: block[0] has no ancestors,
    /// block[i] has block[i-1] as ancestor.
    fn create_pending_chain(len: usize) -> Vec<DeferredTrieData> {
        let mut chain: Vec<DeferredTrieData> = Vec::with_capacity(len);
        for i in 0..len {
            let ancestors = if i == 0 { vec![] } else { vec![chain[i - 1].clone()] };
            chain.push(DeferredTrieData::pending(
                Arc::new(HashedPostState::default()),
                Arc::new(TrieUpdates::default()),
                B256::with_last_byte(i as u8),
                ancestors,
            ));
        }
        chain
    }

    /// Verifies linear ancestor chain completes without deadlock.
    /// Chain: block[n] -> block[n-1] -> ... -> block[0] (no ancestors)
    #[test]
    fn linear_chain_no_deadlock() {
        let chain = create_pending_chain(5);

        // Access the tip - should recursively compute all ancestors without deadlock
        let result = chain[4].wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(4)));
    }

    /// Verifies deep ancestor chain (20+ levels) completes without stack overflow.
    #[test]
    fn deep_chain_no_stack_overflow() {
        let chain = create_pending_chain(25);

        let result = chain[24].wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(24)));
    }

    /// Verifies concurrent access to different blocks in same chain works correctly.
    #[test]
    fn concurrent_chain_access() {
        let chain = create_pending_chain(10);

        // Multiple threads access different points in the chain simultaneously
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let block = chain[i].clone();
                thread::spawn(move || block.wait_cloned())
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each result should have its block's anchor hash
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(i as u8)));
        }
    }

    /// Verifies diamond dependency pattern works: D depends on B and C, both depend on A.
    ///
    /// ```text
    ///       A (no ancestors)
    ///      / \
    ///     B   C
    ///      \ /
    ///       D
    /// ```
    #[test]
    fn diamond_dependency_no_deadlock() {
        // A has no ancestors
        let a = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(0xA),
            vec![],
        );

        // B and C both depend on A
        let b = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(0xB),
            vec![a.clone()],
        );
        let c = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(0xC),
            vec![a.clone()],
        );

        // D depends on both B and C (diamond merge)
        let d = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(0xD),
            vec![b, c],
        );

        // Should complete without deadlock
        let result = d.wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(0xD)));
    }

    /// Verifies two independent chains can be computed in parallel.
    #[test]
    fn parallel_independent_chains() {
        let chain1 = create_pending_chain(5);
        let chain2 = create_pending_chain(5);

        let tip1 = chain1[4].clone();
        let tip2 = chain2[4].clone();

        let h1 = thread::spawn(move || tip1.wait_cloned());
        let h2 = thread::spawn(move || tip2.wait_cloned());

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // Both should complete with their respective anchors
        assert_eq!(r1.anchor_hash(), Some(B256::with_last_byte(4)));
        assert_eq!(r2.anchor_hash(), Some(B256::with_last_byte(4)));
    }

    /// Verifies duplicate ancestor in list doesn't deadlock.
    /// The same ancestor appearing twice should work: first computes, second returns cached.
    #[test]
    fn duplicate_ancestor_no_deadlock() {
        let ancestor = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(1),
            vec![],
        );

        // Block with same ancestor listed twice
        let block = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(2),
            vec![ancestor.clone(), ancestor], // Same ancestor twice!
        );

        // Should complete without deadlock
        let result = block.wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(2)));
    }
}
