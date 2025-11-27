use alloy_primitives::B256;
use parking_lot::Mutex;
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
    /// Number of times deferred trie data was ready (async task completed first).
    deferred_trie_async_ready: Counter,
    /// Number of times deferred trie data required synchronous computation (fallback path).
    deferred_trie_sync_fallback: Counter,
}

static DEFERRED_TRIE_METRICS: LazyLock<DeferredTrieMetrics> =
    LazyLock::new(DeferredTrieMetrics::default);

/// Shared handle to asynchronously populated trie data.
///
/// Uses `OnceLock` for lock-free access once data is computed.
/// If the deferred task hasn't completed, computes trie data synchronously
/// from stored inputs. After computation, inputs are cleared to release
/// ancestor references and prevent O(n^2) memory retention.
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared inner state containing inputs and result.
    inner: Arc<DeferredTrieDataInner>,
}

/// Inner data structure for deferred trie computation.
struct DeferredTrieDataInner {
    /// Pending inputs for fallback computation. Wrapped in Mutex to allow clearing
    /// after computation, which releases ancestor references.
    inputs: Mutex<Option<PendingInputs>>,
    /// Computed result, set once by async task or fallback. Lock-free reads after init.
    result: OnceLock<ComputedTrieData>,
}

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_ready = self.inner.result.get().is_some();
        f.debug_struct("DeferredTrieData")
            .field("state", &if is_ready { "ready" } else { "pending" })
            .finish()
    }
}

impl DeferredTrieData {
    /// Create a new pending handle with fallback inputs for synchronous computation.
    ///
    /// If the async task hasn't completed when `wait_cloned` is called, the trie data
    /// will be computed synchronously from these inputs. This eliminates deadlock risk.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state from execution
    /// * `trie_updates` - Unsorted trie updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestors` - Deferred trie data from ancestor blocks for merging
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        ancestors: Vec<Self>,
    ) -> Self {
        Self {
            inner: Arc::new(DeferredTrieDataInner {
                inputs: Mutex::new(Some(PendingInputs {
                    hashed_state,
                    trie_updates,
                    anchor_hash,
                    ancestors,
                })),
                result: OnceLock::new(),
            }),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately.
    /// [`Self::wait_cloned`] will return without any computation.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        let result = OnceLock::new();
        let _ = result.set(bundle);
        Self { inner: Arc::new(DeferredTrieDataInner { inputs: Mutex::new(None), result }) }
    }

    /// Sort block execution outputs and build a [`TrieInputSorted`] overlay.
    ///
    /// The trie input overlay accumulates sorted hashed state (account/storage changes) and
    /// trie node updates from all in-memory ancestor blocks. This overlay is required for:
    /// - Computing state roots on top of in-memory blocks
    /// - Generating storage/account proofs for unpersisted state
    ///
    /// # Process
    /// 1. Sort the current block's hashed state and trie updates
    /// 2. Merge ancestor overlays (oldest -> newest, so later state takes precedence)
    /// 3. Extend the merged overlay with this block's sorted data
    ///
    /// Used by both the async background task and the synchronous fallback path.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state (account/storage changes) from execution
    /// * `trie_updates` - Unsorted trie node updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestors` - Deferred trie data from ancestor blocks for merging
    pub fn sort_and_build_trie_input(
        hashed_state: &HashedPostState,
        trie_updates: &TrieUpdates,
        anchor_hash: B256,
        ancestors: &[Self],
    ) -> ComputedTrieData {
        // Sort the current block's hashed state and trie updates
        let sorted_hashed_state = Arc::new(hashed_state.clone().into_sorted());
        let sorted_trie_updates = Arc::new(trie_updates.clone().into_sorted());

        // Merge trie data from ancestors (oldest -> newest so later state takes precedence)
        let mut overlay = TrieInputSorted::default();
        for ancestor in ancestors {
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
            anchor_hash,
            Arc::new(overlay),
        )
    }

    /// Compute trie data from inputs and mark the handle as ready.
    ///
    /// Sorts and builds trie input from the provided state and updates, then marks the handle
    /// as ready so that [`Self::wait_cloned`] returns immediately without fallback computation.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state from execution
    /// * `trie_updates` - Unsorted trie updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestors` - Deferred trie data from ancestor blocks for merging
    pub fn compute_set_ready(
        &self,
        hashed_state: &HashedPostState,
        trie_updates: &TrieUpdates,
        anchor_hash: B256,
        ancestors: &[Self],
    ) {
        let bundle =
            Self::sort_and_build_trie_input(hashed_state, trie_updates, anchor_hash, ancestors);
        self.set_ready(bundle);
    }

    /// Populate the handle with the computed trie data.
    ///
    /// Safe to call multiple times; only the first value is stored (first-write-wins).
    /// Clears inputs after setting to release ancestor references.
    pub fn set_ready(&self, bundle: ComputedTrieData) {
        let _ = self.inner.result.set(bundle);
        // Clear inputs to release ancestor references (fixes O(n^2) memory retention)
        let _ = self.inner.inputs.lock().take();
    }

    /// Returns trie data, computing synchronously if the async task hasn't completed.
    ///
    /// Uses `OnceLock`'s `get_or_init` pattern:
    /// - If the async task has completed, returns the cached result (lock-free).
    /// - If pending, computes synchronously from stored inputs.
    ///
    /// After computation, inputs are cleared to release ancestor references.
    /// This design eliminates deadlock risk: we never block waiting for another task.
    /// All code paths are guaranteed to return (either cached or computed result).
    pub fn wait_cloned(&self) -> ComputedTrieData {
        // Fast path: lock-free read if already computed
        if let Some(result) = self.inner.result.get() {
            DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
            return result.clone();
        }

        // Lock inputs for computation
        let mut inputs_guard = self.inner.inputs.lock();

        // Re-check after acquiring lock - another thread may have computed while we waited
        if let Some(result) = self.inner.result.get() {
            DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
            return result.clone();
        }

        // Not yet computed - compute from inputs
        DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);

        let inputs = inputs_guard.as_ref().expect("inputs must be present for pending state");

        // Compute and set result
        let result = self
            .inner
            .result
            .get_or_init(|| {
                Self::sort_and_build_trie_input(
                    &inputs.hashed_state,
                    &inputs.trie_updates,
                    inputs.anchor_hash,
                    &inputs.ancestors,
                )
            })
            .clone();

        // Clear inputs while still holding the lock to prevent races
        *inputs_guard = None;

        result
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

    /// Verifies that `set_ready` value takes precedence when called before `wait_cloned`.
    #[test]
    fn set_ready_wins_over_fallback() {
        let deferred = empty_pending();

        let bundle = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(42),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };

        // Set ready before wait_cloned
        deferred.set_ready(bundle);

        let result = deferred.wait_cloned();
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(42)));
    }

    /// Verifies first-write-wins semantics: only the first `set_ready` value is stored.
    #[test]
    fn multiple_set_ready_takes_first() {
        let deferred = empty_pending();

        let first = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(1),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };
        let second = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(2),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };

        deferred.set_ready(first.clone());
        deferred.set_ready(second);

        assert_eq!(deferred.wait_cloned().anchor_hash(), first.anchor_hash());
    }

    /// Verifies that cloned handles share the same underlying state.
    #[test]
    fn clones_share_state() {
        let deferred = empty_pending();
        let setter = deferred.clone();

        let bundle = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(3),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };

        thread::spawn(move || setter.set_ready(bundle));

        // Give the thread time to set
        thread::sleep(Duration::from_millis(10));
        assert_eq!(deferred.wait_cloned().anchor_hash(), Some(B256::with_last_byte(3)));
    }

    /// Verifies that calling `set_ready` before `wait_cloned` returns the set value immediately.
    #[test]
    fn set_before_wait_returns_set_value() {
        let deferred = empty_pending();

        let bundle = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(4),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };

        deferred.set_ready(bundle.clone());

        let start = Instant::now();
        let result = deferred.wait_cloned();
        let elapsed = start.elapsed();

        assert_eq!(result.anchor_hash(), bundle.anchor_hash());
        assert!(elapsed < Duration::from_millis(20));
    }

    /// Verifies race condition handling: either async `set_ready` or fallback result is returned.
    #[test]
    fn async_set_ready_race_with_fallback() {
        // Test that when async task sets ready, either the set value or fallback is returned
        let deferred = empty_pending_with_anchor(B256::with_last_byte(100)); // Fallback anchor
        let deferred_clone = deferred.clone();

        // Spawn async task that sets ready after a delay
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            let bundle = ComputedTrieData {
                anchored_trie_input: Some(AnchoredTrieInput {
                    anchor_hash: B256::with_last_byte(200), // Async anchor
                    trie_input: Arc::new(TrieInputSorted::default()),
                }),
                ..empty_bundle()
            };
            deferred_clone.set_ready(bundle);
        });

        // Wait a bit for potential race
        thread::sleep(Duration::from_millis(10));

        let result = deferred.wait_cloned();
        // Result should be either from set_ready (200) or fallback (100)
        let anchor = result.anchor_hash();
        assert!(
            anchor == Some(B256::with_last_byte(200)) || anchor == Some(B256::with_last_byte(100)),
            "Expected anchor 100 or 200, got {:?}",
            anchor
        );
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

    /// Verifies concurrent access from multiple threads returns consistent results.
    #[test]
    fn concurrent_wait_cloned_returns_same_result() {
        let deferred = empty_pending_with_anchor(B256::with_last_byte(42));
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let d = deferred.clone();
                thread::spawn(move || d.wait_cloned())
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All results should have the same anchor hash
        let first_anchor = results[0].anchor_hash();
        for result in &results {
            assert_eq!(result.anchor_hash(), first_anchor);
        }
    }

    /// Verifies that inputs are cleared after computation to release ancestor references.
    #[test]
    fn inputs_cleared_after_computation() {
        let ancestor = DeferredTrieData::ready(empty_bundle());
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::ZERO,
            vec![ancestor],
        );

        // Before wait_cloned, inputs should be present
        assert!(deferred.inner.inputs.lock().is_some());

        // Trigger computation
        let _ = deferred.wait_cloned();

        // After wait_cloned, inputs should be cleared
        assert!(deferred.inner.inputs.lock().is_none());
    }

    /// Verifies that set_ready clears inputs.
    #[test]
    fn set_ready_clears_inputs() {
        let ancestor = DeferredTrieData::ready(empty_bundle());
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::ZERO,
            vec![ancestor],
        );

        // Before set_ready, inputs should be present
        assert!(deferred.inner.inputs.lock().is_some());

        // Set ready
        deferred.set_ready(empty_bundle());

        // After set_ready, inputs should be cleared
        assert!(deferred.inner.inputs.lock().is_none());
    }
}
