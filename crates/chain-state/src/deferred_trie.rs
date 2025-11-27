use alloy_primitives::B256;
use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted, TrieInputSorted,
};
use std::{
    cell::Cell,
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

/// Shared handle to asynchronously populated trie data.
///
/// Uses `OnceLock` for exactly-once computation semantics:
/// - Only one thread computes the result (others block waiting)
/// - No lock held during computation
/// - First-write-wins semantics built into `get_or_init()`
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared inner state containing inputs and the result.
    inner: Arc<Inner>,
}

/// Inner state shared by all clones.
struct Inner {
    /// Inputs for fallback computation. `None` for `ready()` handles.
    inputs: Option<PendingInputs>,
    /// Computed result, set exactly once.
    result: OnceLock<ComputedTrieData>,
}

static DEFERRED_TRIE_METRICS: LazyLock<DeferredTrieMetrics> =
    LazyLock::new(DeferredTrieMetrics::default);

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.inner.result.get().is_some() { "ready" } else { "pending" };
        f.debug_struct("DeferredTrieData").field("state", &state).finish()
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
    /// * `parent` - Deferred trie data from parent block (contains cumulative grandparent data)
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        parent: Option<Self>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                inputs: Some(PendingInputs { hashed_state, trie_updates, anchor_hash, parent }),
                result: OnceLock::new(),
            }),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately.
    /// [`Self::wait_cloned`] will return without any computation.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { inner: Arc::new(Inner { inputs: None, result: OnceLock::from(bundle) }) }
    }

    /// Populate the handle with the computed trie data.
    ///
    /// Safe to call multiple times; only the first value is stored (first-write-wins).
    pub fn set_ready(&self, bundle: ComputedTrieData) {
        let _ = self.inner.result.set(bundle); // Ok if already set
    }

    /// Compute trie data from pending inputs and mark the handle as ready.
    ///
    /// This method is designed for background tasks. For pending handles, it computes
    /// and sets the result using `get_or_init()`. For ready handles (inputs=None),
    /// this is a no-op.
    pub fn compute_and_set_ready(&self) {
        // For pending handles, compute and set the result.
        // For ready handles (inputs=None), this is a no-op.
        if let Some(inputs) = &self.inner.inputs {
            let _ = self.inner.result.get_or_init(|| {
                Self::compute_trie_data(
                    &inputs.hashed_state,
                    &inputs.trie_updates,
                    inputs.anchor_hash,
                    inputs.parent.as_ref(),
                )
            });
        }
    }

    /// Returns trie data, computing synchronously if the async task hasn't completed.
    ///
    /// Uses `OnceLock::get_or_init()` for exactly-once computation:
    /// - If already computed, returns cached result immediately
    /// - If not computed, one thread computes while others block waiting
    /// - Metrics accurately track whether this call triggered computation
    ///
    /// For `ready()` handles (inputs=None), the result is pre-populated and returned directly.
    pub fn wait_cloned(&self) -> ComputedTrieData {
        // Ready handles have no inputs and pre-populated result - return directly
        let Some(inputs) = &self.inner.inputs else {
            DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
            return self.inner.result.get().expect("ready handle must have result").clone();
        };

        // Use Cell to track if THIS thread computed the value.
        // - Each thread has its own stack-local Cell
        // - OnceLock ensures only one thread's closure runs
        // - Other threads block, get cached result, their Cell stays false
        let computed_here = Cell::new(false);

        let result = self.inner.result.get_or_init(|| {
            computed_here.set(true);
            Self::compute_trie_data(
                &inputs.hashed_state,
                &inputs.trie_updates,
                inputs.anchor_hash,
                inputs.parent.as_ref(),
            )
        });

        if computed_here.get() {
            DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);
        } else {
            DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
        }

        result.clone()
    }

    /// Sort block execution outputs and build a [`TrieInputSorted`] overlay.
    ///
    /// The trie input overlay accumulates sorted hashed state (account/storage changes) and
    /// trie node updates from all ancestor blocks in the chain.
    ///
    /// # Non-recursive design
    /// This implementation avoids deep recursion by directly accessing unsorted data from
    /// pending ancestors instead of calling `wait_cloned()` on them. This is critical for
    /// long chains where recursive `wait_cloned()` calls would cause stack overflow or
    /// long blocking times.
    ///
    /// # Process
    /// 1. Sort the current block's hashed state and trie updates
    /// 2. Traverse parent chain:
    ///    - For Ready ancestors: use `trie_input` as base (already has all their ancestors)
    ///    - For Pending ancestors: collect their UNSORTED data directly (no `wait_cloned`)
    /// 3. Sort each pending ancestor's data ourselves
    /// 4. Build cumulative overlay from base + sorted ancestor data + current data
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state (account/storage changes) from execution
    /// * `trie_updates` - Unsorted trie node updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `parent` - Parent block's deferred trie data handle
    fn compute_trie_data(
        hashed_state: &HashedPostState,
        trie_updates: &TrieUpdates,
        anchor_hash: B256,
        parent: Option<&Self>,
    ) -> ComputedTrieData {
        // Sort the current block's hashed state and trie updates
        let sorted_hashed_state = Arc::new(hashed_state.clone().into_sorted());
        let sorted_trie_updates = Arc::new(trie_updates.clone().into_sorted());

        // Traverse parent chain, collecting data without triggering recursive computation.
        // For Pending ancestors, we grab their UNSORTED data directly.
        // For Ready ancestors, we use their already-computed trie_input as base.
        let mut pending_unsorted: Vec<(Arc<HashedPostState>, Arc<TrieUpdates>)> = Vec::new();
        let mut base_trie_input: Option<Arc<TrieInputSorted>> = None;
        let mut current = parent.cloned();

        while let Some(p) = current {
            // Check if this ancestor is ready (has computed result)
            if let Some(data) = p.inner.result.get() {
                // Ready: use its trie_input as base, stop traversing.
                // The Ready block's trie_input already contains all its ancestors' data
                // PLUS its own sorted data.
                base_trie_input = data.trie_input().cloned();
                break;
            }

            // Not ready (pending): grab unsorted data directly - NO wait_cloned, NO recursion!
            if let Some(inputs) = &p.inner.inputs {
                let unsorted_state = inputs.hashed_state.clone();
                let unsorted_trie = inputs.trie_updates.clone();
                let next_parent = inputs.parent.clone();
                pending_unsorted.push((unsorted_state, unsorted_trie));
                current = next_parent;
            } else {
                // Ready handle with no inputs (created via ready()) - shouldn't happen in chain
                break;
            }
        }

        // Build overlay starting from base (if any Ready ancestor was found)
        let mut overlay = base_trie_input.map(|arc| (*arc).clone()).unwrap_or_default();

        // Sort and extend with pending ancestors' data (oldest to newest = reverse order)
        for (unsorted_state, unsorted_trie) in pending_unsorted.into_iter().rev() {
            let sorted_state = unsorted_state.as_ref().clone().into_sorted();
            let sorted_trie = unsorted_trie.as_ref().clone().into_sorted();
            Arc::make_mut(&mut overlay.state).extend_ref(&sorted_state);
            Arc::make_mut(&mut overlay.nodes).extend_ref(&sorted_trie);
        }

        // Extend overlay with current block's sorted data
        Arc::make_mut(&mut overlay.state).extend_ref(sorted_hashed_state.as_ref());
        Arc::make_mut(&mut overlay.nodes).extend_ref(sorted_trie_updates.as_ref());

        ComputedTrieData::with_trie_input(
            sorted_hashed_state,
            sorted_trie_updates,
            anchor_hash,
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
    /// Parent block's deferred trie data. Parent's `trie_input` already contains
    /// cumulative grandparent data, enabling O(1) merge per block.
    parent: Option<DeferredTrieData>,
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
            None,
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

    /// Tests that parent trie data is used during fallback computation and that the
    /// resulting `ComputedTrieData` uses the current block's anchor hash, not the parent's.
    #[test]
    fn parent_trie_data_is_used() {
        // Create parent with some data
        let parent_bundle = ComputedTrieData {
            hashed_state: Arc::default(),
            trie_updates: Arc::default(),
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(1),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
        };
        let parent = DeferredTrieData::ready(parent_bundle);

        // Create pending with parent
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            B256::with_last_byte(2),
            Some(parent),
        );

        let result = deferred.wait_cloned();
        // Should have the current block's anchor, not the parent's
        assert_eq!(result.anchor_hash(), Some(B256::with_last_byte(2)));
    }

    /// Tests that parent state is extended with current block's state.
    #[test]
    fn parent_state_is_extended() {
        let key = B256::with_last_byte(1);

        // Parent has nonce 1
        let parent_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );
        let parent_overlay =
            TrieInputSorted { state: Arc::new(parent_state), ..Default::default() };
        let parent_bundle = ComputedTrieData {
            hashed_state: Arc::default(),
            trie_updates: Arc::default(),
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::ZERO,
                trie_input: Arc::new(parent_overlay),
            }),
        };
        let parent = DeferredTrieData::ready(parent_bundle);

        // Current block updates nonce to 2
        let mut current_state = HashedPostState::default();
        current_state
            .accounts
            .insert(key, Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }));

        let deferred = DeferredTrieData::pending(
            Arc::new(current_state),
            Arc::new(TrieUpdates::default()),
            B256::ZERO,
            Some(parent),
        );

        let result = deferred.wait_cloned();
        let overlay_state = &result.anchored_trie_input.as_ref().unwrap().trie_input.state.accounts;
        assert_eq!(overlay_state.len(), 1);
        let (_, account) = &overlay_state[0];
        // Current block's state should take precedence
        assert_eq!(account.unwrap().nonce, 2);
    }

    /// Verifies that `compute_and_set_ready()` is a no-op for ready handles.
    #[test]
    fn compute_and_set_ready_is_noop_for_ready_handles() {
        let bundle = ComputedTrieData {
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash: B256::with_last_byte(42),
                trie_input: Arc::new(TrieInputSorted::default()),
            }),
            ..empty_bundle()
        };
        let deferred = DeferredTrieData::ready(bundle.clone());

        // Should not panic or modify state
        deferred.compute_and_set_ready();

        let result = deferred.wait_cloned();
        assert_eq!(result.anchor_hash(), bundle.anchor_hash());
    }
}
