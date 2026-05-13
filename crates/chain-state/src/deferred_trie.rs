use parking_lot::Mutex;
use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted,
};
use std::{
    fmt,
    sync::{Arc, LazyLock},
};
use tracing::{debug_span, instrument};

/// Shared handle to asynchronously populated per-block trie data.
///
/// If the background task has not completed by the time trie data is needed, the caller computes
/// the sorted data synchronously from the retained unsorted inputs and caches the result.
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared deferred state holding either raw inputs (pending) or computed result (ready).
    state: Arc<Mutex<DeferredState>>,
}

/// Sorted trie data computed for one executed block.
///
/// Cumulative overlays are intentionally managed by
/// [`TrieOverlayManager`](crate::TrieOverlayManager), not by each block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
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
    ///
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
    pub fn pending(hashed_state: Arc<HashedPostState>, trie_updates: Arc<TrieUpdates>) -> Self {
        Self {
            state: Arc::new(Mutex::new(DeferredState::Pending(Some(PendingInputs {
                hashed_state,
                trie_updates,
            })))),
        }
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { state: Arc::new(Mutex::new(DeferredState::Ready(bundle))) }
    }

    /// Sorts block execution outputs.
    pub fn sort(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
    ) -> ComputedTrieData {
        let _span = debug_span!(target: "engine::tree::deferred_trie", "sort_inputs").entered();

        #[cfg(feature = "rayon")]
        let (sorted_hashed_state, sorted_trie_updates) = rayon::join(
            || match Arc::try_unwrap(hashed_state) {
                Ok(state) => state.into_sorted(),
                Err(arc) => arc.clone_into_sorted(),
            },
            || match Arc::try_unwrap(trie_updates) {
                Ok(updates) => updates.into_sorted(),
                Err(arc) => arc.clone_into_sorted(),
            },
        );

        #[cfg(not(feature = "rayon"))]
        let (sorted_hashed_state, sorted_trie_updates) = (
            match Arc::try_unwrap(hashed_state) {
                Ok(state) => state.into_sorted(),
                Err(arc) => arc.clone_into_sorted(),
            },
            match Arc::try_unwrap(trie_updates) {
                Ok(updates) => updates.into_sorted(),
                Err(arc) => arc.clone_into_sorted(),
            },
        );

        ComputedTrieData::new(Arc::new(sorted_hashed_state), Arc::new(sorted_trie_updates))
    }

    /// Returns trie data, computing synchronously if the async task hasn't completed.
    #[instrument(level = "debug", target = "engine::tree::deferred_trie", skip_all)]
    pub fn wait_cloned(&self) -> ComputedTrieData {
        let mut state = self.state.lock();
        match &mut *state {
            DeferredState::Ready(bundle) => {
                DEFERRED_TRIE_METRICS.deferred_trie_async_ready.increment(1);
                bundle.clone()
            }
            DeferredState::Pending(maybe_inputs) => {
                DEFERRED_TRIE_METRICS.deferred_trie_sync_fallback.increment(1);

                let inputs = maybe_inputs.take().expect("inputs must be present in Pending state");
                let computed = Self::sort(inputs.hashed_state, inputs.trie_updates);
                *state = DeferredState::Ready(computed.clone());

                computed
            }
        }
    }
}

impl ComputedTrieData {
    /// Construct sorted trie data for one block.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates }
    }

    /// Construct sorted trie data for one block.
    pub const fn without_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self::new(hashed_state, trie_updates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::B256Map, B256, U256};
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdates, HashedStorage};
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn empty_pending() -> DeferredTrieData {
        DeferredTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
        )
    }

    #[test]
    fn ready_returns_immediately() {
        let bundle = ComputedTrieData::default();
        let deferred = DeferredTrieData::ready(bundle.clone());

        let result = deferred.wait_cloned();

        assert_eq!(result.hashed_state.total_len(), bundle.hashed_state.total_len());
        assert_eq!(result.trie_updates.total_len(), bundle.trie_updates.total_len());
    }

    #[test]
    fn pending_computes_and_caches_result() {
        let deferred = empty_pending();

        let first = deferred.wait_cloned();
        let second = deferred.wait_cloned();

        assert!(Arc::ptr_eq(&first.hashed_state, &second.hashed_state));
        assert!(Arc::ptr_eq(&first.trie_updates, &second.trie_updates));
    }

    #[test]
    fn concurrent_waits_share_computed_result() {
        let deferred = empty_pending();
        let deferred2 = deferred.clone();

        let handle = thread::spawn(move || deferred2.wait_cloned());
        let result1 = deferred.wait_cloned();
        let result2 = handle.join().unwrap();

        assert!(Arc::ptr_eq(&result1.hashed_state, &result2.hashed_state));
        assert!(Arc::ptr_eq(&result1.trie_updates, &result2.trie_updates));
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

        let deferred =
            DeferredTrieData::pending(Arc::new(hashed_state), Arc::new(TrieUpdates::default()));
        let result = deferred.wait_cloned();

        assert_eq!(result.hashed_state.total_len(), 2);
        assert_eq!(result.trie_updates.total_len(), 0);
    }

    #[test]
    fn wait_does_not_block_after_first_compute() {
        let mut accounts = B256Map::default();
        for i in 0..100 {
            accounts.insert(B256::with_last_byte(i), Some(Account::default()));
        }
        let deferred = DeferredTrieData::pending(
            Arc::new(HashedPostState { accounts, storages: Default::default() }),
            Arc::new(TrieUpdates::default()),
        );

        let _ = deferred.wait_cloned();
        let start = Instant::now();
        let _ = deferred.wait_cloned();

        assert!(start.elapsed() < Duration::from_millis(10));
    }
}
