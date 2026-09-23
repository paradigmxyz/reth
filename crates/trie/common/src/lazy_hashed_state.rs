//! Sorted hashed state published by a background task.

use crate::{HashedPostState, HashedPostStateSorted};
use alloc::sync::Arc;
use core::fmt;
use reth_primitives_traits::sync::OnceLock;

/// Shared sorted hashed state, available immediately or published by a background task.
/// Clones share the same result and wait for the producer when it is still pending.
#[derive(Clone)]
pub struct LazyHashedPostStateSorted {
    value: Arc<OnceLock<Arc<HashedPostStateSorted>>>,
}

impl LazyHashedPostStateSorted {
    /// Creates a handle to already sorted hashed state.
    pub fn ready(state: Arc<HashedPostStateSorted>) -> Self {
        Self { value: Arc::new(OnceLock::from(state)) }
    }

    /// Creates a handle and a producer that sorts and publishes the hashed state.
    #[cfg(feature = "std")]
    pub fn pending(hashed_state: Arc<HashedPostState>) -> (Self, HashedPostStateSortedProducer) {
        let value = Arc::new(OnceLock::new());
        (Self { value: Arc::clone(&value) }, HashedPostStateSortedProducer { value, hashed_state })
    }

    /// Returns the sorted hashed state, waiting for its producer if it is still pending.
    pub fn get(&self) -> &Arc<HashedPostStateSorted> {
        #[cfg(feature = "std")]
        {
            self.value.wait()
        }
        #[cfg(not(feature = "std"))]
        {
            self.value.get().expect("hashed state must be initialized")
        }
    }
}

impl fmt::Debug for LazyHashedPostStateSorted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyHashedPostStateSorted")
            .field("initialized", &self.value.get().is_some())
            .finish()
    }
}

impl PartialEq for LazyHashedPostStateSorted {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl Eq for LazyHashedPostStateSorted {}

#[cfg(feature = "serde")]
impl serde::Serialize for LazyHashedPostStateSorted {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.get().serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for LazyHashedPostStateSorted {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Arc::deserialize(deserializer).map(Self::ready)
    }
}

/// Sorts hashed state and publishes it to waiting handles.
#[derive(Debug)]
#[must_use = "call compute_and_publish to wake hashed state waiters"]
pub struct HashedPostStateSortedProducer {
    value: Arc<OnceLock<Arc<HashedPostStateSorted>>>,
    hashed_state: Arc<HashedPostState>,
}

impl HashedPostStateSortedProducer {
    /// Sorts hashed state, publishes it to waiters, and returns the shared result.
    pub fn compute_and_publish(self) -> Arc<HashedPostStateSorted> {
        let sorted = Arc::new(match Arc::try_unwrap(self.hashed_state) {
            Ok(state) => state.into_sorted(),
            Err(state) => state.clone_into_sorted(),
        });
        let _ = self.value.set(Arc::clone(&sorted));
        sorted
    }
}

#[cfg(test)]
mod tests {
    use crate::HashedStorage;

    use super::*;
    use alloy_primitives::{map::B256Map, B256, U256};
    use reth_primitives_traits::Account;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn empty_pending() -> (LazyHashedPostStateSorted, HashedPostStateSortedProducer) {
        LazyHashedPostStateSorted::pending(Arc::new(HashedPostState::default()))
    }

    #[test]
    fn test_lazy_ready_is_initialized() {
        let lazy = LazyHashedPostStateSorted::ready(Arc::default());
        let _ = lazy.get();
    }

    #[test]
    fn test_lazy_clone_shares_state() {
        let lazy1 = LazyHashedPostStateSorted::ready(Arc::default());
        let lazy2 = lazy1.clone();

        // Both point to the same data
        assert!(Arc::ptr_eq(lazy1.get(), lazy2.get()));
    }

    #[test]
    fn ready_returns_immediately() {
        let bundle = Arc::new(HashedPostStateSorted::default());
        let deferred = LazyHashedPostStateSorted::ready(bundle.clone());

        let result = deferred.get();

        assert_eq!(result.total_len(), bundle.total_len());
    }

    #[test]
    fn pending_waits_for_task_and_caches_result() {
        let (deferred, task) = empty_pending();
        let published = task.compute_and_publish();
        let first = deferred.get();
        let second = deferred.get();

        assert!(Arc::ptr_eq(&published, first));
        assert!(Arc::ptr_eq(first, second));
    }

    #[test]
    fn pending_wait_blocks_until_task_publishes() {
        let (deferred, task) = empty_pending();

        let handle = thread::spawn(move || deferred.get().clone());
        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());

        let published = task.compute_and_publish();
        let result = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published, &result));
    }

    #[test]
    fn concurrent_waits_share_published_result() {
        let (deferred, task) = empty_pending();
        let deferred2 = deferred.clone();

        let handle = thread::spawn(move || deferred2.get().clone());
        let published = task.compute_and_publish();
        let result1 = deferred.get().clone();
        let result2 = handle.join().unwrap();

        assert!(Arc::ptr_eq(&published, &result1));
        assert!(Arc::ptr_eq(&result1, &result2));
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

        let (deferred, task) = LazyHashedPostStateSorted::pending(Arc::new(hashed_state));
        let _ = task.compute_and_publish();
        let result = deferred.get().clone();

        assert_eq!(result.total_len(), 2);
    }

    #[test]
    fn wait_does_not_block_after_first_compute() {
        let mut accounts = B256Map::default();
        for i in 0..100 {
            accounts.insert(B256::with_last_byte(i), Some(Account::default()));
        }
        let (deferred, task) = LazyHashedPostStateSorted::pending(Arc::new(HashedPostState {
            accounts,
            storages: Default::default(),
        }));

        let _ = task.compute_and_publish();
        let _ = deferred.get().clone();
        let start = Instant::now();
        let _ = deferred.get().clone();

        assert!(start.elapsed() < Duration::from_millis(10));
    }
}
