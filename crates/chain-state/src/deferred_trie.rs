use alloy_primitives::B256;
use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    prefix_set::TriePrefixSetsMut,
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted, TrieInputSorted,
};
use std::{
    fmt,
    sync::{Arc, LazyLock, OnceLock},
};
use tracing::{debug_span, instrument};

/// Shared handle to asynchronously populated sorted trie data.
///
/// The corresponding [`DeferredTrieDataProducer`] owns the unsorted inputs and publishes the sorted
/// data when the background task completes. Callers wait for that result instead of computing it
/// synchronously.
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared deferred result populated by the corresponding [`DeferredTrieDataProducer`].
    value: Arc<OnceLock<ComputedTrieData>>,
}

/// Producer consumed by a spawned task to compute sorted trie data for a [`DeferredTrieData`]
/// handle.
#[must_use = "DeferredTrieDataProducer must be consumed with compute_and_publish to wake trie data waiters"]
pub struct DeferredTrieDataProducer {
    /// Shared result initialized exactly once by this producer.
    value: Arc<OnceLock<ComputedTrieData>>,
    /// Unsorted inputs consumed when the producer computes trie data.
    inputs: PendingInputs,
}

impl fmt::Debug for DeferredTrieDataProducer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredTrieDataProducer")
            .field("inputs", &self.inputs)
            .finish_non_exhaustive()
    }
}

impl DeferredTrieDataProducer {
    /// Computes sorted trie data, publishes it to waiters, and returns it to the task owner.
    pub fn compute_and_publish(self) -> ComputedTrieData {
        let Self { value, inputs } = self;
        let computed = DeferredTrieData::sort_and_build_trie_input(
            inputs.hashed_state,
            inputs.trie_updates,
            inputs.changed_paths,
            inputs.anchor_hash,
            &inputs.ancestors,
        );
        let _ = value.set(computed.clone());
        computed
    }
}

/// Sorted trie data computed for an executed block.
///
/// This includes the per-block sorted hashed state and trie updates required for persistence and
/// proof generation. When available, `anchored_trie_input` contains the cumulative in-memory trie
/// input rooted at a persisted ancestor.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// Changed trie node base paths produced by state root computation.
    pub changed_paths: Option<Arc<TriePrefixSetsMut>>,
    /// Trie input bundled with its anchor hash, if available.
    pub anchored_trie_input: Option<AnchoredTrieInput>,
}

/// Trie input bundled with its anchor hash.
///
/// The `trie_input` contains the cumulative overlay of all in-memory ancestor blocks, not just this
/// block's changes. The anchor identifies the persisted base state this overlay sits on top of.
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
    /// Number of times deferred trie data was ready when requested.
    deferred_trie_async_ready: Counter,
    /// Number of times deferred trie data required waiting for the publishing task.
    deferred_trie_task_wait: Counter,
}

static DEFERRED_TRIE_METRICS: LazyLock<DeferredTrieMetrics> =
    LazyLock::new(DeferredTrieMetrics::default);

/// Inputs kept while a deferred trie computation is pending.
#[derive(Clone, Debug)]
struct PendingInputs {
    /// Unsorted hashed post-state from execution.
    hashed_state: Arc<HashedPostState>,
    /// Unsorted trie updates from state root computation.
    trie_updates: Arc<TrieUpdates>,
    /// Changed trie node base paths from state root computation.
    changed_paths: Option<Arc<TriePrefixSetsMut>>,
    /// The persisted ancestor hash this trie input is anchored to.
    anchor_hash: B256,
    /// Deferred trie data from ancestor blocks for merging, oldest to newest.
    ancestors: Vec<DeferredTrieData>,
}

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredTrieData")
            .field("state", &if self.value.get().is_some() { "ready" } else { "pending" })
            .finish()
    }
}

impl DeferredTrieData {
    /// Create a new pending handle and task that will publish the computed trie data.
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
        anchor_hash: B256,
        ancestors: Vec<Self>,
    ) -> (Self, DeferredTrieDataProducer) {
        let value = Arc::new(OnceLock::new());
        (
            Self { value: Arc::clone(&value) },
            DeferredTrieDataProducer {
                value,
                inputs: PendingInputs {
                    hashed_state,
                    trie_updates,
                    changed_paths,
                    anchor_hash,
                    ancestors,
                },
            },
        )
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { value: Arc::new(OnceLock::from(bundle)) }
    }

    /// Sorts block execution outputs without building an accumulated trie input overlay.
    pub fn sort(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
    ) -> ComputedTrieData {
        let (hashed_state, trie_updates) = Self::sort_inputs(hashed_state, trie_updates);
        ComputedTrieData::new_with_changed_paths(
            Arc::new(hashed_state),
            Arc::new(trie_updates),
            changed_paths,
        )
    }

    /// Sort block execution outputs and build a [`TrieInputSorted`] overlay.
    pub fn sort_and_build_trie_input(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
        anchor_hash: B256,
        ancestors: &[Self],
    ) -> ComputedTrieData {
        let (sorted_hashed_state, sorted_trie_updates) =
            Self::sort_inputs(hashed_state, trie_updates);

        let _span = debug_span!(target: "engine::tree::deferred_trie", "build_overlay").entered();

        let overlay = if let Some(parent) = ancestors.last() {
            let parent_data = parent.wait_cloned();
            match &parent_data.anchored_trie_input {
                Some(AnchoredTrieInput { anchor_hash: parent_anchor, trie_input })
                    if *parent_anchor == anchor_hash =>
                {
                    let mut overlay = TrieInputSorted::new(
                        Arc::clone(&trie_input.nodes),
                        Arc::clone(&trie_input.state),
                        Default::default(),
                    );
                    Self::extend_overlay(&mut overlay, &sorted_hashed_state, &sorted_trie_updates);
                    overlay
                }
                _ => Self::merge_ancestors_into_overlay(
                    ancestors,
                    &sorted_hashed_state,
                    &sorted_trie_updates,
                ),
            }
        } else {
            Self::merge_ancestors_into_overlay(&[], &sorted_hashed_state, &sorted_trie_updates)
        };

        ComputedTrieData::with_trie_input_and_changed_paths(
            Arc::new(sorted_hashed_state),
            Arc::new(sorted_trie_updates),
            changed_paths,
            anchor_hash,
            Arc::new(overlay),
        )
    }

    fn sort_inputs(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
    ) -> (HashedPostStateSorted, TrieUpdatesSorted) {
        let _span = debug_span!(target: "engine::tree::deferred_trie", "sort_inputs").entered();

        #[cfg(feature = "rayon")]
        {
            rayon::join(
                || match Arc::try_unwrap(hashed_state) {
                    Ok(state) => state.into_sorted(),
                    Err(arc) => arc.clone_into_sorted(),
                },
                || match Arc::try_unwrap(trie_updates) {
                    Ok(updates) => updates.into_sorted(),
                    Err(arc) => arc.clone_into_sorted(),
                },
            )
        }

        #[cfg(not(feature = "rayon"))]
        {
            (
                match Arc::try_unwrap(hashed_state) {
                    Ok(state) => state.into_sorted(),
                    Err(arc) => arc.clone_into_sorted(),
                },
                match Arc::try_unwrap(trie_updates) {
                    Ok(updates) => updates.into_sorted(),
                    Err(arc) => arc.clone_into_sorted(),
                },
            )
        }
    }

    fn extend_overlay(
        overlay: &mut TrieInputSorted,
        sorted_hashed_state: &HashedPostStateSorted,
        sorted_trie_updates: &TrieUpdatesSorted,
    ) {
        let _span = debug_span!(target: "engine::tree::deferred_trie", "extend_overlay").entered();

        #[cfg(feature = "rayon")]
        {
            rayon::join(
                || {
                    if !sorted_hashed_state.is_empty() {
                        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(sorted_hashed_state);
                    }
                },
                || {
                    if !sorted_trie_updates.is_empty() {
                        Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(sorted_trie_updates);
                    }
                },
            );
        }

        #[cfg(not(feature = "rayon"))]
        {
            if !sorted_hashed_state.is_empty() {
                Arc::make_mut(&mut overlay.state).extend_ref_and_sort(sorted_hashed_state);
            }
            if !sorted_trie_updates.is_empty() {
                Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(sorted_trie_updates);
            }
        }
    }

    /// Merge all ancestors and current block's data into a single overlay.
    #[cfg(feature = "rayon")]
    fn merge_ancestors_into_overlay(
        ancestors: &[Self],
        sorted_hashed_state: &HashedPostStateSorted,
        sorted_trie_updates: &TrieUpdatesSorted,
    ) -> TrieInputSorted {
        if ancestors.is_empty() {
            return TrieInputSorted::new(
                Arc::new(sorted_trie_updates.clone()),
                Arc::new(sorted_hashed_state.clone()),
                Default::default(),
            );
        }

        let (states, updates): (Vec<_>, Vec<_>) = ancestors
            .iter()
            .rev()
            .map(|ancestor| {
                let data = ancestor.wait_cloned();
                (data.hashed_state, data.trie_updates)
            })
            .unzip();

        let (state, nodes) = rayon::join(
            || {
                let mut merged = HashedPostStateSorted::merge_slice(&states);
                merged.extend_ref_and_sort(sorted_hashed_state);
                merged
            },
            || {
                let mut merged = TrieUpdatesSorted::merge_slice(&updates);
                merged.extend_ref_and_sort(sorted_trie_updates);
                merged
            },
        );

        TrieInputSorted::new(Arc::new(nodes), Arc::new(state), Default::default())
    }

    /// Sequential fallback when rayon is not available.
    #[cfg(not(feature = "rayon"))]
    fn merge_ancestors_into_overlay(
        ancestors: &[Self],
        sorted_hashed_state: &HashedPostStateSorted,
        sorted_trie_updates: &TrieUpdatesSorted,
    ) -> TrieInputSorted {
        let _span = debug_span!(
            target: "engine::tree::deferred_trie",
            "merge_ancestors",
            num_ancestors = ancestors.len()
        )
        .entered();
        let mut overlay = TrieInputSorted::default();
        {
            let state_mut = Arc::make_mut(&mut overlay.state);
            for ancestor in ancestors {
                let ancestor_data = ancestor.wait_cloned();
                state_mut.extend_ref_and_sort(ancestor_data.hashed_state.as_ref());
            }
            state_mut.extend_ref_and_sort(sorted_hashed_state);
        }
        {
            let nodes_mut = Arc::make_mut(&mut overlay.nodes);
            for ancestor in ancestors {
                let ancestor_data = ancestor.wait_cloned();
                nodes_mut.extend_ref_and_sort(ancestor_data.trie_updates.as_ref());
            }
            nodes_mut.extend_ref_and_sort(sorted_trie_updates);
        }
        overlay
    }

    /// Returns trie data, waiting for the async publishing task if it has not completed.
    #[instrument(level = "debug", target = "engine::tree::deferred_trie", skip_all)]
    pub fn wait_cloned(&self) -> ComputedTrieData {
        let bundle = match self.value.get() {
            Some(bundle) => {
                DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
                bundle
            }
            None => {
                DEFERRED_TRIE_METRICS.deferred_trie_task_wait.increment(1);
                self.value.wait()
            }
        };

        bundle.clone()
    }
}

impl ComputedTrieData {
    /// Construct sorted trie data without an accumulated trie input overlay.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self::new_with_changed_paths(hashed_state, trie_updates, None)
    }

    /// Construct sorted trie data with changed trie node base paths for one block.
    pub const fn new_with_changed_paths(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
    ) -> Self {
        Self { hashed_state, trie_updates, changed_paths, anchored_trie_input: None }
    }

    /// Construct a bundle that includes trie input anchored to a persisted ancestor.
    pub const fn with_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        anchor_hash: B256,
        trie_input: Arc<TrieInputSorted>,
    ) -> Self {
        Self::with_trie_input_and_changed_paths(
            hashed_state,
            trie_updates,
            None,
            anchor_hash,
            trie_input,
        )
    }

    /// Construct a bundle that includes changed paths and trie input.
    pub const fn with_trie_input_and_changed_paths(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
        anchor_hash: B256,
        trie_input: Arc<TrieInputSorted>,
    ) -> Self {
        Self {
            hashed_state,
            trie_updates,
            changed_paths,
            anchored_trie_input: Some(AnchoredTrieInput { anchor_hash, trie_input }),
        }
    }

    /// Construct a bundle without trie input or anchor information.
    pub const fn without_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self::new(hashed_state, trie_updates)
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
    use reth_trie::{updates::TrieUpdates, HashedStorage};
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn empty_bundle() -> ComputedTrieData {
        ComputedTrieData::default()
    }

    fn empty_pending() -> (DeferredTrieData, DeferredTrieDataProducer) {
        DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            None,
            B256::ZERO,
            Vec::new(),
        )
    }

    fn publish(
        pending: (DeferredTrieData, DeferredTrieDataProducer),
    ) -> (DeferredTrieData, ComputedTrieData) {
        let (deferred, task) = pending;
        let published = task.compute_and_publish();
        (deferred, published)
    }

    fn assert_changed_paths_ptr_eq(
        left: &Option<Arc<TriePrefixSetsMut>>,
        right: &Option<Arc<TriePrefixSetsMut>>,
    ) {
        match (left, right) {
            (Some(left), Some(right)) => assert!(Arc::ptr_eq(left, right)),
            (None, None) => {}
            _ => panic!("changed paths presence mismatch"),
        }
    }

    fn pending_ready(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
        anchor_hash: B256,
        ancestors: Vec<DeferredTrieData>,
    ) -> DeferredTrieData {
        let (deferred, task) =
            DeferredTrieData::pending(hashed_state, trie_updates, None, anchor_hash, ancestors);
        let _ = task.compute_and_publish();
        deferred
    }

    fn ready_block_with_state(
        anchor_hash: B256,
        accounts: Vec<(B256, Option<Account>)>,
    ) -> DeferredTrieData {
        let hashed_state = Arc::new(HashedPostStateSorted::new(accounts, B256Map::default()));
        let trie_updates = Arc::default();
        let mut overlay = TrieInputSorted::default();
        Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state.as_ref());

        DeferredTrieData::ready(ComputedTrieData::with_trie_input(
            hashed_state,
            trie_updates,
            anchor_hash,
            Arc::new(overlay),
        ))
    }

    #[test]
    fn ready_returns_immediately() {
        let bundle = empty_bundle();
        let deferred = DeferredTrieData::ready(bundle.clone());

        let start = Instant::now();
        let result = deferred.wait_cloned();

        assert_eq!(result.hashed_state, bundle.hashed_state);
        assert_eq!(result.trie_updates, bundle.trie_updates);
        assert_eq!(result.anchor_hash(), bundle.anchor_hash());
        assert!(start.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn pending_waits_for_task_and_caches_result() {
        let (deferred, published) = publish(empty_pending());

        let first = deferred.wait_cloned();
        let second = deferred.wait_cloned();

        assert!(Arc::ptr_eq(&published.hashed_state, &first.hashed_state));
        assert!(Arc::ptr_eq(&published.trie_updates, &first.trie_updates));
        assert_changed_paths_ptr_eq(&published.changed_paths, &first.changed_paths);
        assert!(Arc::ptr_eq(&first.hashed_state, &second.hashed_state));
        assert!(Arc::ptr_eq(&first.trie_updates, &second.trie_updates));
        assert_changed_paths_ptr_eq(&first.changed_paths, &second.changed_paths);
        assert_eq!(first.anchor_hash(), second.anchor_hash());
    }

    #[test]
    fn pending_wait_blocks_until_task_publishes() {
        let (deferred, task) = empty_pending();

        let handle = thread::spawn(move || deferred.wait_cloned());
        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());

        let published = task.compute_and_publish();
        let result = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published.hashed_state, &result.hashed_state));
        assert!(Arc::ptr_eq(&published.trie_updates, &result.trie_updates));
        assert_changed_paths_ptr_eq(&published.changed_paths, &result.changed_paths);
    }

    #[test]
    fn sorts_non_empty_inputs() {
        let hashed_address = B256::with_last_byte(1);
        let hashed_slot = B256::with_last_byte(2);
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(1))]),
            )]);

        let (deferred, task) = DeferredTrieData::pending(
            Arc::new(hashed_state),
            Arc::new(TrieUpdates::default()),
            None,
            B256::ZERO,
            Vec::new(),
        );
        let _ = task.compute_and_publish();
        let result = deferred.wait_cloned();

        assert_eq!(result.hashed_state.total_len(), 2);
        assert_eq!(result.trie_updates.total_len(), 0);
    }

    #[test]
    fn ancestors_merge_in_chronological_order() {
        let key = B256::with_last_byte(1);
        let oldest_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );
        let newest_state = HashedPostStateSorted::new(
            vec![(key, Some(Account { nonce: 2, balance: U256::ZERO, bytecode_hash: None }))],
            B256Map::default(),
        );

        let oldest = ComputedTrieData::without_trie_input(
            Arc::new(oldest_state),
            Arc::new(TrieUpdatesSorted::default()),
        );
        let newest = ComputedTrieData::without_trie_input(
            Arc::new(newest_state),
            Arc::new(TrieUpdatesSorted::default()),
        );

        let deferred = pending_ready(
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

    #[test]
    fn first_block_after_anchor_creates_base_overlay() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None };

        let first_block = pending_ready(
            Arc::new(HashedPostState::default().with_accounts([(key, Some(account))])),
            Arc::new(TrieUpdates::default()),
            anchor,
            Vec::new(),
        );

        let result = first_block.wait_cloned();
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 1);
    }

    #[test]
    fn reuses_parent_overlay_when_anchor_matches() {
        let anchor = B256::with_last_byte(1);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 100, balance: U256::ZERO, bytecode_hash: None };
        let parent = ready_block_with_state(anchor, vec![(key, Some(account))]);

        let child = pending_ready(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            anchor,
            vec![parent],
        );

        let result = child.wait_cloned();
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 100);
    }

    #[test]
    fn rebuilds_overlay_when_anchor_changes() {
        let old_anchor = B256::with_last_byte(1);
        let new_anchor = B256::with_last_byte(2);
        let key = B256::with_last_byte(42);
        let account = Account { nonce: 50, balance: U256::ZERO, bytecode_hash: None };
        let parent = ready_block_with_state(old_anchor, vec![(key, Some(account))]);

        let child = pending_ready(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            new_anchor,
            vec![parent],
        );

        let result = child.wait_cloned();
        let overlay = result.anchored_trie_input.as_ref().unwrap();
        assert_eq!(overlay.anchor_hash, new_anchor);
        assert_eq!(overlay.trie_input.state.accounts.len(), 1);
        let (found_key, found_account) = &overlay.trie_input.state.accounts[0];
        assert_eq!(*found_key, key);
        assert_eq!(found_account.unwrap().nonce, 50);
    }

    #[test]
    fn changed_paths_are_preserved() {
        let changed_paths = Arc::new(TriePrefixSetsMut::default());
        let (deferred, task) = DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            Some(Arc::clone(&changed_paths)),
            B256::ZERO,
            Vec::new(),
        );

        let published = task.compute_and_publish();
        let result = deferred.wait_cloned();

        assert!(Arc::ptr_eq(published.changed_paths.as_ref().unwrap(), &changed_paths));
        assert_changed_paths_ptr_eq(&published.changed_paths, &result.changed_paths);
    }
}
