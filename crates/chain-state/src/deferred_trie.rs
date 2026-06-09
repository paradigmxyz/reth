use reth_metrics::{metrics::Counter, Metrics};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted,
};
use std::{
    fmt,
    sync::{Arc, LazyLock, OnceLock},
};
use tracing::{debug_span, instrument};

/// Shared handle to asynchronously populated sorted per-block trie data.
///
/// The corresponding [`DeferredTrieDataProducer`] owns the unsorted inputs and publishes the sorted
/// data when the background task completes. Callers wait for that result instead of computing it
/// synchronously.
#[derive(Clone)]
pub struct DeferredTrieData {
    /// Shared deferred result populated by the corresponding [`DeferredTrieDataProducer`].
    value: Arc<OnceLock<ComputedTrieData>>,
}

/// Producer consumed by a spawned task to compute sorted trie data for a [`DeferredTrieData`] handle.
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

/// Sorted trie data computed for one executed block.
///
/// Cumulative overlays are intentionally managed by
/// [`StateTrieOverlayManager`](crate::StateTrieOverlayManager), not by each block.
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
}

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredTrieData")
            .field("state", &if self.value.get().is_some() { "ready" } else { "pending" })
            .finish()
    }
}

impl DeferredTrieDataProducer {
    /// Computes sorted trie data, publishes it to waiters, and returns it to the task owner.
    pub fn compute_and_publish(self) -> ComputedTrieData {
        let Self { value, inputs } = self;
        let computed = DeferredTrieData::sort(inputs.hashed_state, inputs.trie_updates);
        let _ = value.set(computed.clone());
        computed
    }
}

impl DeferredTrieData {
    /// Create a new pending handle and task that will publish the computed trie data.
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
    ) -> (Self, DeferredTrieDataProducer) {
        let value = Arc::new(OnceLock::new());
        (
            Self { value: Arc::clone(&value) },
            DeferredTrieDataProducer { value, inputs: PendingInputs { hashed_state, trie_updates } },
        )
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    pub fn ready(bundle: ComputedTrieData) -> Self {
        Self { value: Arc::new(OnceLock::from(bundle)) }
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
    /// Construct sorted trie data for one block.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates }
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

    fn empty_pending() -> (DeferredTrieData, DeferredTrieDataProducer) {
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
    fn pending_waits_for_task_and_caches_result() {
        let (deferred, task) = empty_pending();

        let published = task.compute_and_publish();
        let first = deferred.wait_cloned();
        let second = deferred.wait_cloned();

        assert!(Arc::ptr_eq(&published.hashed_state, &first.hashed_state));
        assert!(Arc::ptr_eq(&published.trie_updates, &first.trie_updates));
        assert!(Arc::ptr_eq(&first.hashed_state, &second.hashed_state));
        assert!(Arc::ptr_eq(&first.trie_updates, &second.trie_updates));
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
    }

    #[test]
    fn concurrent_waits_share_published_result() {
        let (deferred, task) = empty_pending();
        let deferred2 = deferred.clone();

        let handle = thread::spawn(move || deferred2.wait_cloned());
        let published = task.compute_and_publish();
        let result1 = deferred.wait_cloned();
        let result2 = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published.hashed_state, &result1.hashed_state));
        assert!(Arc::ptr_eq(&published.trie_updates, &result1.trie_updates));
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

        let (deferred, task) =
            DeferredTrieData::pending(Arc::new(hashed_state), Arc::new(TrieUpdates::default()));
        let _ = task.compute_and_publish();
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
        let (deferred, task) = DeferredTrieData::pending(
            Arc::new(HashedPostState { accounts, storages: Default::default() }),
            Arc::new(TrieUpdates::default()),
        );

        let _ = task.compute_and_publish();
        let _ = deferred.wait_cloned();
        let start = Instant::now();
        let _ = deferred.wait_cloned();

        assert!(start.elapsed() < Duration::from_millis(10));
    }
}
