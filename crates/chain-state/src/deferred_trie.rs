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
use tracing::instrument;

/// Shared handle to asynchronously populated trie data.
///
/// Uses a try-lock + fallback computation approach for deadlock-free access.
/// If the deferred task hasn't completed, computes trie data synchronously
/// from stored unsorted inputs rather than blocking.
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared deferred state holding either raw inputs (pending) or computed result (ready).
    state: Arc<Mutex<DeferredState>>,
}

/// Sorted trie data computed for an executed block.
/// These represent the complete set of sorted trie data required to persist
/// block state for, and generate proofs on top of, a block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// Trie input bundled with its anchor hash, if available.
    pub anchored_trie_input: Option<AnchoredTrieInput>,
}

/// Trie input bundled with its anchor hash.
///
/// The `trie_input` contains the **cumulative** overlay of all in-memory ancestor blocks,
/// not just this block's changes. Child blocks reuse the parent's overlay in O(1) by
/// cloning the Arc-wrapped data.
///
/// The `anchor_hash` is metadata indicating which persisted base state this overlay
/// sits on top of. It is CRITICAL for overlay reuse decisions: an overlay built on top
/// of Anchor A cannot be reused for a block anchored to Anchor B, as it would result
/// in an incorrect state.
#[derive(Clone, Debug)]
pub struct AnchoredTrieInput {
    /// The persisted ancestor hash this trie input is anchored to.
    pub anchor_hash: B256,
    /// Cumulative trie input overlay from all in-memory ancestors.
    pub trie_input: Arc<TrieInputSorted>,
}

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

/// Internal state for deferred trie data.
enum DeferredState {
    /// Data is not yet available; raw inputs stored for fallback computation.
    /// Wrapped in `Option` to allow taking ownership during computation.
    Pending(Option<PendingInputs>),
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
            state: Arc::new(Mutex::new(DeferredState::Pending(Some(PendingInputs {
                hashed_state,
                trie_updates,
                anchor_hash,
                ancestors,
            })))),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately.
    /// [`Self::wait_cloned`] will return without any computation.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { state: Arc::new(Mutex::new(DeferredState::Ready(bundle))) }
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
    /// 2. Reuse parent's cached overlay if available (O(1) - the common case)
    /// 3. Otherwise, rebuild overlay from ancestors (rare fallback)
    /// 4. Extend the overlay with this block's sorted data
    ///
    /// Used by both the async background task and the synchronous fallback path.
    ///
    /// # Arguments
    /// * `hashed_state` - Unsorted hashed post-state (account/storage changes) from execution
    /// * `trie_updates` - Unsorted trie node updates from state root computation
    /// * `anchor_hash` - The persisted ancestor hash this trie input is anchored to
    /// * `ancestors` - Deferred trie data from ancestor blocks for merging (oldest -> newest)
    pub fn sort_and_build_trie_input(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        ancestors: &[Self],
    ) -> ComputedTrieData {
        let sorted_hashed_state = match Arc::try_unwrap(hashed_state) {
            Ok(state) => state.into_sorted(),
            Err(arc) => arc.clone_into_sorted(),
        };
        let sorted_trie_updates = match Arc::try_unwrap(trie_updates) {
            Ok(updates) => updates.into_sorted(),
            Err(arc) => arc.clone_into_sorted(),
        };

        // Reuse parent's overlay if available and anchors match.
        // We can only reuse the parent's overlay if it was built on top of the same
        // persisted anchor. If the anchor has changed (e.g., due to persistence),
        // the parent's overlay is relative to an old state and cannot be used.
        let overlay = if let Some(parent) = ancestors.last() {
            let parent_data = parent.wait_cloned();

            match &parent_data.anchored_trie_input {
                // Case 1: Parent has cached overlay AND anchors match.
                Some(AnchoredTrieInput { anchor_hash: parent_anchor, trie_input })
                    if *parent_anchor == anchor_hash =>
                {
                    // O(1): Reuse parent's overlay, extend with current block's data.
                    let mut overlay = TrieInputSorted::new(
                        Arc::clone(&trie_input.nodes),
                        Arc::clone(&trie_input.state),
                        Default::default(), // prefix_sets are per-block, not cumulative
                    );
                    // Only trigger COW clone if there's actually data to add.
                    if !sorted_hashed_state.is_empty() {
                        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(&sorted_hashed_state);
                    }
                    if !sorted_trie_updates.is_empty() {
                        Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(&sorted_trie_updates);
                    }
                    overlay
                }
                // Case 2: Parent exists but anchor mismatch or no cached overlay.
                // We must rebuild from the ancestors list (which only contains unpersisted blocks).
                _ => Self::merge_ancestors_into_overlay(
                    ancestors,
                    &sorted_hashed_state,
                    &sorted_trie_updates,
                ),
            }
        } else {
            // Case 3: No in-memory ancestors (first block after persisted anchor).
            // Build overlay with just this block's data.
            Self::merge_ancestors_into_overlay(&[], &sorted_hashed_state, &sorted_trie_updates)
        };

        ComputedTrieData::with_trie_input(
            Arc::new(sorted_hashed_state),
            Arc::new(sorted_trie_updates),
            anchor_hash,
            Arc::new(overlay),
        )
    }

    /// Merge all ancestors and current block's data into a single overlay.
    ///
    /// This is a rare fallback path, only used when no ancestor has a cached
    /// `anchored_trie_input` (e.g., blocks created via alternative constructors).
    /// In normal operation, the parent always has a cached overlay and this
    /// function is never called.
    ///
    /// Iterates ancestors oldest -> newest, then extends with current block's data,
    /// so later state takes precedence.
    fn merge_ancestors_into_overlay(
        ancestors: &[Self],
        sorted_hashed_state: &HashedPostStateSorted,
        sorted_trie_updates: &TrieUpdatesSorted,
    ) -> TrieInputSorted {
        let mut overlay = TrieInputSorted::default();

        let state_mut = Arc::make_mut(&mut overlay.state);
        let nodes_mut = Arc::make_mut(&mut overlay.nodes);

        for ancestor in ancestors {
            let ancestor_data = ancestor.wait_cloned();
            state_mut.extend_ref_and_sort(ancestor_data.hashed_state.as_ref());
            nodes_mut.extend_ref_and_sort(ancestor_data.trie_updates.as_ref());
        }

        // Extend with current block's sorted data last (takes precedence)
        state_mut.extend_ref_and_sort(sorted_hashed_state);
        nodes_mut.extend_ref_and_sort(sorted_trie_updates);

        overlay
    }

    /// Returns trie data, computing synchronously if the async task hasn't completed.
    ///
    /// - If the async task has completed (`Ready`), returns the cached result.
    /// - If pending, computes synchronously from stored inputs.
    ///
    /// Deadlock is avoided as long as the provided ancestors form a true ancestor chain (a DAG):
    /// - Each block only waits on its ancestors (blocks on the path to the persisted root)
    /// - Sibling blocks (forks) are never in each other's ancestor lists
    /// - A block never waits on its descendants
    ///
    /// Given that invariant, circular wait dependencies are impossible.
    #[instrument(level = "debug", target = "engine::tree::deferred_trie", skip_all)]
    pub fn wait_cloned(&self) -> ComputedTrieData {
        let mut state = self.state.lock();
        match &mut *state {
            // If the deferred trie data is ready, return the cached result.
            DeferredState::Ready(bundle) => {
                DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
                bundle.clone()
            }
            // If the deferred trie data is pending, compute the trie data synchronously and return
            // the result. This is the fallback path if the async task hasn't completed.
            DeferredState::Pending(maybe_inputs) => {
                DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);

                let inputs = maybe_inputs.take().expect("inputs must be present in Pending state");

                let computed = Self::sort_and_build_trie_input(
                    inputs.hashed_state,
                    inputs.trie_updates,
                    inputs.anchor_hash,
                    &inputs.ancestors,
                );
                *state = DeferredState::Ready(computed.clone());
                computed
            }
        }
    }
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
    ///
    /// Unlike [`Self::with_trie_input`], this constructor omits the accumulated trie input overlay
    /// and its anchor hash. Use this when the trie input is not needed, such as in block builders
    /// or sequencers that don't require proof generation on top of in-memory state.
    ///
    /// The trie input anchor identifies the persisted block hash from which the in-memory overlay
    /// was built. Without it, consumers cannot determine which on-disk state to combine with.
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

    /// Verifies that concurrent `wait_cloned` calls result in only one computation,
    /// with all callers receiving the same cached result.
    #[test]
    fn concurrent_wait_cloned_computes_once() {
        let deferred = empty_pending();

        // Spawn multiple threads that all call wait_cloned concurrently
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let d = deferred.clone();
                thread::spawn(move || d.wait_cloned())
            })
            .collect();

        // Collect all results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All results should share the same Arc pointers (same computed result)
        let first = &results[0];
        for result in &results[1..] {
            assert!(Arc::ptr_eq(&first.hashed_state, &result.hashed_state));
            assert!(Arc::ptr_eq(&first.trie_updates, &result.trie_updates));
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

    /// Helper to create a ready block with anchored trie input containing specific state.
    fn ready_block_with_state(
        anchor_hash: B256,
        accounts: Vec<(B256, Option<Account>)>,
    ) -> DeferredTrieData {
        let hashed_state = Arc::new(HashedPostStateSorted::new(accounts, B256Map::default()));
        let trie_updates = Arc::default();
        let mut overlay = TrieInputSorted::default();
        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state.as_ref());

        DeferredTrieData::ready(ComputedTrieData {
            hashed_state,
            trie_updates,
            anchored_trie_input: Some(AnchoredTrieInput {
                anchor_hash,
                trie_input: Arc::new(overlay),
            }),
        })
    }

    /// Verifies that first block after anchor (no ancestors) creates empty base overlay.
    #[test]
    fn first_block_after_anchor_creates_empty_base() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None };

        // First block after anchor - no ancestors
        let first_block = DeferredTrieData::pending(
            Arc::new(HashedPostState::default().with_accounts([(key, Some(account))])),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![], // No ancestors
        );

        let result = first_block.wait_cloned();

        // Should have overlay with just this block's data
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 1);
    }

    /// Verifies that parent's overlay is reused regardless of anchor.
    #[test]
    fn reuses_parent_overlay() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 100, balance: U256::ZERO, bytecode_hash: None };

        // Create parent with anchored trie input
        let parent = ready_block_with_state(anchor, vec![(key, Some(account))]);

        // Create child - should reuse parent's overlay
        let child = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![parent],
        );

        let result = child.wait_cloned();

        // Verify parent's account is in the overlay
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 100);
    }

    /// Verifies that parent's overlay is NOT reused when anchor changes (after persist).
    /// The overlay data is dependent on the anchor, so it must be rebuilt from the
    /// remaining ancestors.
    #[test]
    fn rebuilds_overlay_when_anchor_changes() {
        let old_anchor = B256::with_last_byte(1);
        let new_anchor = B256::with_last_byte(2);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 50, balance: U256::ZERO, bytecode_hash: None };

        // Create parent with OLD anchor
        let parent = ready_block_with_state(old_anchor, vec![(key, Some(account))]);

        // Create child with NEW anchor (simulates after persist)
        // Should NOT reuse parent's overlay because anchor changed
        let child = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            new_anchor,
            vec![parent],
        );

        let result = child.wait_cloned();

        // Verify result uses new anchor
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, new_anchor);

        // Crucially, since we provided `parent` in ancestors but it has a different anchor,
        // the code falls back to `merge_ancestors_into_overlay`.
        // `merge_ancestors_into_overlay` reads `parent.hashed_state` (which has the account).
        // So the account IS present, but it was obtained via REBUILD, not REUSE.
        // We can check `DEFERRED_TRIE_METRICS` if we want to be sure, but functionally:
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 50);
    }

    /// Verifies that parent without `anchored_trie_input` triggers rebuild path.
    #[test]
    fn rebuilds_when_parent_has_no_anchored_input() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 25, balance: U256::ZERO, bytecode_hash: None };

        // Create parent WITHOUT anchored trie input (e.g., from without_trie_input constructor)
        let parent_state =
            HashedPostStateSorted::new(vec![(key, Some(account))], B256Map::default());
        let parent = DeferredTrieData::ready(ComputedTrieData {
            hashed_state: Arc::new(parent_state),
            trie_updates: Arc::default(),
            anchored_trie_input: None, // No anchored input
        });

        // Create child - should rebuild from parent's hashed_state
        let child = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![parent],
        );

        let result = child.wait_cloned();

        // Verify overlay is built and contains parent's data
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
    }

    /// Verifies that a chain of blocks with matching anchors builds correct cumulative overlay.
    #[test]
    fn chain_of_blocks_builds_cumulative_overlay() {
        let anchor = B256::with_last_byte(1);
        let key1 = B256::with_last_byte(1);
        let key2 = B256::with_last_byte(2);
        let key3 = B256::with_last_byte(3);

        // Block 1: sets account at key1
        let block1 = ready_block_with_state(
            anchor,
            vec![(key1, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
        );

        // Block 2: adds account at key2, ancestor is block1
        let block2_hashed = HashedPostState::default().with_accounts([(
            key2,
            Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let block2 = DeferredTrieData::pending(
            Arc::new(block2_hashed),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![block1.clone()],
        );
        // Compute block2's trie data
        let block2_computed = block2.wait_cloned();
        let block2_ready = DeferredTrieData::ready(block2_computed);

        // Block 3: adds account at key3, ancestor is block2 (which includes block1)
        let block3_hashed = HashedPostState::default().with_accounts([(
            key3,
            Some(Account { nonce: 3, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let block3 = DeferredTrieData::pending(
            Arc::new(block3_hashed),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![block1, block2_ready],
        );

        let result = block3.wait_cloned();

        // Verify all three accounts are in the cumulative overlay
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.trie_input.state.accounts.len(), 3);

        // Accounts should be sorted by key (B256 ordering)
        let accounts = &overlay.trie_input.state.accounts;
        assert!(accounts.iter().any(|(k, a)| *k == key1 && a.unwrap().nonce == 1));
        assert!(accounts.iter().any(|(k, a)| *k == key2 && a.unwrap().nonce == 2));
        assert!(accounts.iter().any(|(k, a)| *k == key3 && a.unwrap().nonce == 3));
    }

    /// Verifies that child block's state overwrites parent's state for the same key.
    #[test]
    fn child_state_overwrites_parent() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);

        // Parent sets nonce to 10
        let parent = ready_block_with_state(
            anchor,
            vec![(key, Some(Account { nonce: 10, balance: U256::ZERO, bytecode_hash: None }))],
        );

        // Child overwrites nonce to 99
        let child_hashed = HashedPostState::default().with_accounts([(
            key,
            Some(Account { nonce: 99, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let child = DeferredTrieData::pending(
            Arc::new(child_hashed),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![parent],
        );

        let result = child.wait_cloned();

        // Verify child's value wins (extend_ref uses later value)
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        // Note: extend_ref may result in duplicate keys; check the last occurrence
        let accounts = &overlay.trie_input.state.accounts;
        let last_account = accounts.iter().rfind(|(k, _)| *k == key).unwrap();
        assert_eq!(last_account.1.unwrap().nonce, 99);
    }

    /// Stress test: verify O(N) behavior by building a chain of many blocks.
    /// This test ensures the fix doesn't regress - previously this would be O(N²).
    #[test]
    fn long_chain_builds_in_linear_time() {
        let anchor = B256::with_last_byte(1);
        let num_blocks = 50; // Enough to notice O(N²) vs O(N) difference

        let mut ancestors: Vec<DeferredTrieData> = Vec::new();

        let start = Instant::now();

        for i in 0..num_blocks {
            let key = B256::with_last_byte(i as u8);
            let account = Account { nonce: i as u64, balance: U256::ZERO, bytecode_hash: None };
            let hashed = HashedPostState::default().with_accounts([(key, Some(account))]);

            let block = DeferredTrieData::pending(
                Arc::new(hashed),
                Arc::new(TrieUpdates::default()),
                anchor,
                ancestors.clone(),
            );

            // Compute and add to ancestors for next iteration
            let computed = block.wait_cloned();
            ancestors.push(DeferredTrieData::ready(computed));
        }

        let elapsed = start.elapsed();

        // With O(N) fix, 50 blocks should complete quickly (< 1 second)
        // With O(N²), this would take significantly longer
        assert!(
            elapsed < Duration::from_secs(2),
            "Chain of {num_blocks} blocks took {:?}, possible O(N²) regression",
            elapsed
        );

        // Verify final overlay has all accounts
        let final_result = ancestors.last().unwrap().wait_cloned();
        let overlay = final_result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.trie_input.state.accounts.len(), num_blocks);
    }

    /// Verifies that a multi-ancestor overlay is rebuilt when anchor changes.
    /// This simulates the "persist prefix then keep building" scenario where:
    /// 1. A chain of blocks is built with anchor A
    /// 2. Some blocks are persisted, changing anchor to B
    /// 3. New blocks must rebuild the overlay from the remaining ancestors
    #[test]
    fn multi_ancestor_overlay_rebuilt_after_anchor_change() {
        let old_anchor = B256::with_last_byte(1);
        let new_anchor = B256::with_last_byte(2);
        let key1 = B256::with_last_byte(1);
        let key2 = B256::with_last_byte(2);
        let key3 = B256::with_last_byte(3);
        let key4 = B256::with_last_byte(4);

        // Build a chain of 3 blocks with old_anchor
        let block1 = ready_block_with_state(
            old_anchor,
            vec![(key1, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
        );

        let block2_hashed = HashedPostState::default().with_accounts([(
            key2,
            Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let block2 = DeferredTrieData::pending(
            Arc::new(block2_hashed),
            Arc::new(TrieUpdates::default()),
            old_anchor,
            vec![block1.clone()],
        );
        let block2_ready = DeferredTrieData::ready(block2.wait_cloned());

        let block3_hashed = HashedPostState::default().with_accounts([(
            key3,
            Some(Account { nonce: 3, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let block3 = DeferredTrieData::pending(
            Arc::new(block3_hashed),
            Arc::new(TrieUpdates::default()),
            old_anchor,
            vec![block1.clone(), block2_ready.clone()],
        );
        let block3_ready = DeferredTrieData::ready(block3.wait_cloned());

        // Verify block3's overlay has all 3 accounts with old_anchor
        let block3_overlay = block3_ready.wait_cloned().anchored_trie_input.unwrap();
        assert_eq!(block3_overlay.anchor_hash, old_anchor);
        assert_eq!(block3_overlay.trie_input.state.accounts.len(), 3);

        // Now simulate persist: create block4 with NEW anchor but same ancestors.
        // To verify correct rebuilding, we must provide ALL unpersisted ancestors.
        // If we only provided block3, the rebuild would only see block3's state.
        // We pass block1, block2, block3 to simulate that they are all still in memory
        // but the anchor check forces a rebuild (e.g. artificial anchor change).
        let block4_hashed = HashedPostState::default().with_accounts([(
            key4,
            Some(Account { nonce: 4, balance: U256::ZERO, bytecode_hash: None }),
        )]);
        let block4 = DeferredTrieData::pending(
            Arc::new(block4_hashed),
            Arc::new(TrieUpdates::default()),
            new_anchor, // Different anchor - simulates post-persist
            vec![block1, block2_ready, block3_ready],
        );

        let result = block4.wait_cloned();

        // Verify:
        // 1. New anchor is used in result
        assert_eq!(result.anchor_hash(), Some(new_anchor));

        // 2. All 4 accounts are in the overlay (rebuilt from ancestors + extended)
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.trie_input.state.accounts.len(), 4);

        // 3. All accounts have correct values
        let accounts = &overlay.trie_input.state.accounts;
        assert!(accounts.iter().any(|(k, a)| *k == key1 && a.unwrap().nonce == 1));
        assert!(accounts.iter().any(|(k, a)| *k == key2 && a.unwrap().nonce == 2));
        assert!(accounts.iter().any(|(k, a)| *k == key3 && a.unwrap().nonce == 3));
        assert!(accounts.iter().any(|(k, a)| *k == key4 && a.unwrap().nonce == 4));
    }
}
