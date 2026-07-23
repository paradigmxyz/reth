//! Lazy initialization wrapper for trie data.
//!
//! Provides a no-std compatible [`LazyTrieData`] type for lazily initialized
//! trie-related data containing sorted hashed state and trie updates.

use crate::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted,
};
use alloc::sync::Arc;
use core::fmt;
use reth_primitives_traits::sync::OnceLock;

/// Container for sorted trie data: hashed state and trie updates.
///
/// This bundles both [`HashedPostStateSorted`] and [`TrieUpdatesSorted`] together
/// for convenient passing and storage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SortedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
}

impl SortedTrieData {
    /// Creates a new [`SortedTrieData`] with the given values.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates }
    }
}

/// Container for sorted trie data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputedTrieData {
    /// Sorted trie data: hashed state and trie updates.
    pub sorted: SortedTrieData,
}

impl ComputedTrieData {
    /// Construct sorted trie data for one block.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { sorted: SortedTrieData::new(hashed_state, trie_updates) }
    }
}

/// Lazily initialized trie data containing sorted hashed state and trie updates.
///
/// This is a no-std compatible wrapper that supports three modes:
/// 1. **Ready mode**: Data is available immediately (created via `ready()`)
/// 2. **Deferred mode**: Data is computed on first access (created via `deferred()`)
/// 3. **Pending mode**: Data is computed in background task, callers wait for that result (created
///    via `pending()`).
///
/// In deferred mode, the computation runs on the first call to `get()`, `hashed_state()`,
/// or `trie_updates()`, and results are cached for subsequent calls.
///
/// Cloning is cheap (Arc clone) and clones share the cached state.
pub struct LazyTrieData {
    /// Cached sorted trie data, computed on first access.
    data: Arc<OnceLock<ComputedTrieData>>,
    // /// Optional deferred computation function.
    // compute: Option<Arc<dyn Fn() -> SortedTrieData + Send + Sync>>,
    /// Lazy mode.
    mode: LazyTrieDataMode,
}

impl Clone for LazyTrieData {
    fn clone(&self) -> Self {
        Self { data: Arc::clone(&self.data), mode: self.mode.clone() }
    }
}

impl fmt::Debug for LazyTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyTrieData")
            .field("data", &if self.data.get().is_some() { "initialized" } else { "pending" })
            .finish()
    }
}

impl PartialEq for LazyTrieData {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl Eq for LazyTrieData {}

impl LazyTrieData {
    /// Creates a new [`LazyTrieData`] that is already initialized with the given values.
    pub fn ready(sorted: ComputedTrieData) -> Self {
        Self { data: Arc::new(OnceLock::from(sorted)), mode: LazyTrieDataMode::Ready }
    }

    /// Creates a new [`LazyTrieData`] with a deferred computation function.
    ///
    /// The computation will run on the first call to `get()`, `hashed_state()`,
    /// or `trie_updates()`. Results are cached for subsequent calls.
    pub fn deferred(compute: impl Fn() -> ComputedTrieData + Send + Sync + 'static) -> Self {
        Self {
            data: Arc::new(OnceLock::new()),
            mode: LazyTrieDataMode::Deferred(Arc::new(compute)),
        }
    }

    /// Creates a new [`LazyTrieData`] with a spawned task to compute sorted trie data.
    #[cfg(feature = "std")]
    pub fn pending(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
    ) -> (Self, LazyTrieDataProducer) {
        let value = Arc::new(OnceLock::new());
        (
            Self { data: Arc::clone(&value), mode: LazyTrieDataMode::Pending },
            LazyTrieDataProducer { value, inputs: PendingInputs { hashed_state, trie_updates } },
        )
    }

    /// Returns a reference to the sorted trie data, computing or waiting for result if necessary.
    ///
    /// # Panics
    ///
    /// Panics if in ready state, but value has not been initialized.
    pub fn get(&self) -> &ComputedTrieData {
        match &self.mode {
            LazyTrieDataMode::Ready => self.data.get().expect("LazyTrieData must be initialized"),
            LazyTrieDataMode::Deferred(compute) => self.data.get_or_init(|| compute.as_ref()()),
            #[cfg(feature = "std")]
            LazyTrieDataMode::Pending => self.data.wait(),
        }
    }

    /// Returns a clone of the hashed state Arc.
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn hashed_state(&self) -> Arc<HashedPostStateSorted> {
        Arc::clone(&self.get().sorted.hashed_state)
    }

    /// Returns a clone of the trie updates Arc.
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn trie_updates(&self) -> Arc<TrieUpdatesSorted> {
        Arc::clone(&self.get().sorted.trie_updates)
    }

    /// Returns a clone of the [`SortedTrieData`].
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn sorted_trie_data(&self) -> SortedTrieData {
        self.get().sorted.clone()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for LazyTrieData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.get().sorted.serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for LazyTrieData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = SortedTrieData::deserialize(deserializer)?;
        Ok(Self::ready(ComputedTrieData::new(data.hashed_state, data.trie_updates)))
    }
}

#[derive(Clone)]
enum LazyTrieDataMode {
    Ready,
    Deferred(Arc<dyn Fn() -> ComputedTrieData + Send + Sync>),
    #[cfg(feature = "std")]
    Pending,
}

impl fmt::Debug for LazyTrieDataMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => write!(f, "Ready"),
            Self::Deferred(_) => write!(f, "Deferred(..)"),
            #[cfg(feature = "std")]
            Self::Pending => write!(f, "Pending"),
        }
    }
}

/// Producer consumed by a spawned task to compute sorted trie data for a [`LazyTrieData`] handle.
#[must_use = "LazyTrieDataProducer must be consumed with compute_and_publish to wake trie data waiters"]
pub struct LazyTrieDataProducer {
    /// Shared result initialized exactly once by this producer.
    value: Arc<OnceLock<ComputedTrieData>>,
    /// Unsorted inputs consumed when the producer computes trie data.
    inputs: PendingInputs,
}

impl fmt::Debug for LazyTrieDataProducer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyTrieDataProducer").field("inputs", &self.inputs).finish_non_exhaustive()
    }
}

impl LazyTrieDataProducer {
    /// Computes sorted trie data, publishes it to waiters, and returns it to the task owner.
    pub fn compute_and_publish(self) -> ComputedTrieData {
        let Self { value, inputs } = self;
        let computed = Self::sort(inputs.hashed_state, inputs.trie_updates);
        let _ = value.set(computed.clone());
        computed
    }

    /// Sorts block execution outputs.
    pub fn sort(
        hashed_state: Arc<HashedPostState>,
        trie_updates: Arc<TrieUpdates>,
    ) -> ComputedTrieData {
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
}

/// Inputs kept while a deferred trie computation is pending.
#[derive(Clone, Debug)]
struct PendingInputs {
    /// Unsorted hashed post-state from execution.
    hashed_state: Arc<HashedPostState>,
    /// Unsorted trie updates from state root computation.
    trie_updates: Arc<TrieUpdates>,
}

#[cfg(test)]
mod tests {
    use crate::HashedStorage;

    use super::*;
    use alloy_primitives::{B256, U256};
    use reth_primitives_traits::Account;
    use revm::primitives::B256Map;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn empty_pending() -> (LazyTrieData, LazyTrieDataProducer) {
        LazyTrieData::pending(
            Arc::new(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
        )
    }

    #[test]
    fn test_lazy_ready_is_initialized() {
        let lazy = LazyTrieData::ready(ComputedTrieData::default());
        let _ = lazy.hashed_state();
        let _ = lazy.trie_updates();
    }

    #[test]
    fn test_lazy_clone_shares_state() {
        let lazy1 = LazyTrieData::ready(ComputedTrieData::default());
        let lazy2 = lazy1.clone();

        // Both point to the same data
        assert!(Arc::ptr_eq(&lazy1.hashed_state(), &lazy2.hashed_state()));
        assert!(Arc::ptr_eq(&lazy1.trie_updates(), &lazy2.trie_updates()));
    }

    #[test]
    fn test_lazy_deferred() {
        let lazy = LazyTrieData::deferred(ComputedTrieData::default);
        assert!(lazy.hashed_state().is_empty());
        assert!(lazy.trie_updates().is_empty());
    }

    #[test]
    fn ready_returns_immediately() {
        let bundle = ComputedTrieData::default();
        let deferred = LazyTrieData::ready(bundle.clone());

        let result = deferred.get();

        assert_eq!(result.sorted.hashed_state.total_len(), bundle.sorted.hashed_state.total_len());
        assert_eq!(result.sorted.trie_updates.total_len(), bundle.sorted.trie_updates.total_len());
    }

    #[test]
    fn pending_waits_for_task_and_caches_result() {
        let (deferred, task) = empty_pending();

        let published = task.compute_and_publish();
        let first = deferred.get();
        let second = deferred.get();

        assert!(Arc::ptr_eq(&published.sorted.hashed_state, &first.sorted.hashed_state));
        assert!(Arc::ptr_eq(&published.sorted.trie_updates, &first.sorted.trie_updates));
        assert!(Arc::ptr_eq(&first.sorted.hashed_state, &second.sorted.hashed_state));
        assert!(Arc::ptr_eq(&first.sorted.trie_updates, &second.sorted.trie_updates));
    }

    #[test]
    fn pending_wait_blocks_until_task_publishes() {
        let (deferred, task) = empty_pending();

        let handle = thread::spawn(move || deferred.get().clone());
        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());

        let published = task.compute_and_publish();
        let result = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published.sorted.hashed_state, &result.sorted.hashed_state));
        assert!(Arc::ptr_eq(&published.sorted.trie_updates, &result.sorted.trie_updates));
    }

    #[test]
    fn concurrent_waits_share_published_result() {
        let (deferred, task) = empty_pending();
        let deferred2 = deferred.clone();

        let handle = thread::spawn(move || deferred2.get().clone());
        let published = task.compute_and_publish();
        let result1 = deferred.get().clone();
        let result2 = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published.sorted.hashed_state, &result1.sorted.hashed_state));
        assert!(Arc::ptr_eq(&published.sorted.trie_updates, &result1.sorted.trie_updates));
        assert!(Arc::ptr_eq(&result1.sorted.hashed_state, &result2.sorted.hashed_state));
        assert!(Arc::ptr_eq(&result1.sorted.trie_updates, &result2.sorted.trie_updates));
    }

    #[test]
    fn sorts_non_empty_inputs() {
        let hashed_address = B256::with_last_byte(1);
        let hashed_slot = B256::with_last_byte(2);
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter([(hashed_slot, U256::from(1))]),
            )]);

        let (deferred, task) =
            LazyTrieData::pending(Arc::new(hashed_state), Arc::new(TrieUpdates::default()));
        let _ = task.compute_and_publish();
        let result = deferred.get().clone();

        assert_eq!(result.sorted.hashed_state.total_len(), 2);
        assert_eq!(result.sorted.trie_updates.total_len(), 0);
    }

    #[test]
    fn wait_does_not_block_after_first_compute() {
        let mut accounts = B256Map::default();
        for i in 0..100 {
            accounts.insert(B256::with_last_byte(i), Some(Account::default()));
        }
        let (deferred, task) = LazyTrieData::pending(
            Arc::new(HashedPostState { accounts, storages: Default::default() }),
            Arc::new(TrieUpdates::default()),
        );

        let _ = task.compute_and_publish();
        let _ = deferred.get().clone();
        let start = Instant::now();
        let _ = deferred.get().clone();

        assert!(start.elapsed() < Duration::from_millis(10));
    }
}
