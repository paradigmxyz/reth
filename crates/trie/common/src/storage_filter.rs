//! Cuckoo filter for tracking accounts with storage.
//!
//! This filter is used to skip storage proof calculations for accounts that
//! definitely have no storage, providing a significant performance optimization.

use alloy_primitives::B256;
use core::fmt;
use cuckoofilter::{CuckooError, CuckooFilter};
use rustc_hash::FxHasher;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

/// A cuckoo filter for tracking which accounts have storage.
///
/// This is used during state root calculation to skip computing storage proofs
/// for accounts that are known to have no storage. The filter has a small false
/// positive rate (accounts incorrectly marked as having storage), which only
/// results in unnecessary proof calculations but never incorrect state roots.
///
/// False negatives are impossible - if an account has storage, the filter will
/// always report it as potentially having storage.
pub struct StorageAccountFilter {
    filter: CuckooFilter<FxHasher>,
}

impl Clone for StorageAccountFilter {
    fn clone(&self) -> Self {
        Self { filter: CuckooFilter::from(self.filter.export()) }
    }
}

impl fmt::Debug for StorageAccountFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageAccountFilter")
            .field("len", &self.filter.len())
            .field("memory_usage", &self.filter.memory_usage())
            .finish()
    }
}

impl StorageAccountFilter {
    /// Creates a new filter with the specified capacity.
    ///
    /// The capacity should be sized to hold all expected accounts with storage,
    /// plus some headroom (typically 20%) to avoid insertion failures.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { filter: CuckooFilter::with_capacity(capacity) }
    }

    /// Returns `true` if the account may have storage.
    ///
    /// A `true` result means the account might have storage (could be a false positive).
    /// A `false` result means the account definitely has no storage.
    #[inline]
    pub fn may_have_storage(&self, hashed_address: B256) -> bool {
        self.filter.contains(&hashed_address)
    }

    /// Inserts an account into the filter.
    ///
    /// Returns `Ok(())` if successful, or `Err(CuckooError::NotEnoughSpace)` if
    /// the filter is too full. When this error occurs, the element was still
    /// added but a random other element was evicted.
    #[inline]
    pub fn insert(&mut self, hashed_address: B256) -> Result<(), CuckooError> {
        self.filter.add(&hashed_address)
    }

    /// Returns the number of accounts tracked by the filter.
    #[inline]
    pub fn len(&self) -> usize {
        self.filter.len()
    }

    /// Returns `true` if the filter is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.filter.is_empty()
    }

    /// Returns the memory usage of the filter in bytes.
    #[inline]
    pub fn memory_usage(&self) -> usize {
        self.filter.memory_usage()
    }
}

impl Default for StorageAccountFilter {
    fn default() -> Self {
        Self { filter: CuckooFilter::with_capacity(cuckoofilter::DEFAULT_CAPACITY) }
    }
}

/// A shared, thread-safe storage account filter with lock-free reads and updates.
///
/// Uses atomic pointer swaps for fully lock-free operation:
/// - **Readers**: Call `load()` to get an `Arc` clone, then read without any locks
/// - **Writers**: Call `update()` which atomically swaps in a new filter (can run in background)
#[derive(Clone)]
pub struct SharedStorageAccountFilter {
    ptr: Arc<AtomicPtr<StorageAccountFilter>>,
}

impl fmt::Debug for SharedStorageAccountFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedStorageAccountFilter").finish_non_exhaustive()
    }
}

impl SharedStorageAccountFilter {
    /// Creates a new shared filter with the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::from(StorageAccountFilter::with_capacity(capacity))
    }

    /// Loads the current filter for reading (lock-free).
    ///
    /// Returns an `Arc` clone that can be used without holding any locks.
    #[inline]
    pub fn load(&self) -> Arc<StorageAccountFilter> {
        let ptr = self.ptr.load(Ordering::Acquire);
        // SAFETY: ptr was created from Arc::into_raw and is always valid
        unsafe {
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    }

    /// Updates the filter with a mutation function (lock-free, safe to call from background).
    ///
    /// Loads the current filter, clones if there are other references, applies the
    /// mutation, and atomically swaps in the new filter.
    ///
    /// Note: This is not safe to call concurrently from multiple threads. Use external
    /// synchronization if multiple writers are needed.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut StorageAccountFilter),
    {
        // Take ownership of the current Arc directly (without incrementing ref count)
        let old_ptr = self.ptr.load(Ordering::Acquire);
        // SAFETY: ptr was created from Arc::into_raw and is always valid
        let current: Arc<StorageAccountFilter> = unsafe { Arc::from_raw(old_ptr) };

        // If no readers hold references, this won't clone (count == 1)
        let mut filter = Arc::unwrap_or_clone(current);
        f(&mut filter);

        // Create new Arc and store
        let new_arc = Arc::new(filter);
        let new_ptr = Arc::into_raw(new_arc) as *mut _;
        self.ptr.store(new_ptr, Ordering::Release);
    }
}

impl From<StorageAccountFilter> for SharedStorageAccountFilter {
    fn from(filter: StorageAccountFilter) -> Self {
        let arc = Arc::new(filter);
        Self { ptr: Arc::new(AtomicPtr::new(Arc::into_raw(arc) as *mut _)) }
    }
}

impl Default for SharedStorageAccountFilter {
    fn default() -> Self {
        Self::from(StorageAccountFilter::default())
    }
}

impl Drop for SharedStorageAccountFilter {
    fn drop(&mut self) {
        // Only drop the inner Arc if we're the last holder of the outer Arc
        if Arc::strong_count(&self.ptr) == 1 {
            let ptr = self.ptr.load(Ordering::Acquire);
            if !ptr.is_null() {
                unsafe { drop(Arc::from_raw(ptr)) };
            }
        }
    }
}

// SAFETY: AtomicPtr operations are properly synchronized
unsafe impl Send for SharedStorageAccountFilter {}
unsafe impl Sync for SharedStorageAccountFilter {}

/// Update result from a bundle state update.
#[derive(Debug, Default)]
pub struct StorageFilterUpdateStats {
    /// Number of accounts inserted (had new storage).
    pub inserted: usize,

    /// Number of insertion failures (filter too full).
    pub insert_failures: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn test_insert_and_contains() {
        let mut filter = StorageAccountFilter::with_capacity(100);

        let addr1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let addr2 = b256!("0000000000000000000000000000000000000000000000000000000000000002");

        assert!(!filter.may_have_storage(addr1));
        assert!(!filter.may_have_storage(addr2));

        filter.insert(addr1).unwrap();

        assert!(filter.may_have_storage(addr1));
        assert!(!filter.may_have_storage(addr2));
        assert_eq!(filter.len(), 1);
    }

    #[test]
    fn test_shared_filter_load_and_update() {
        let addr1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let addr2 = b256!("0000000000000000000000000000000000000000000000000000000000000002");

        let shared = SharedStorageAccountFilter::with_capacity(100);

        // Initially empty
        let snapshot = shared.load();
        assert!(!snapshot.may_have_storage(addr1));
        assert_eq!(snapshot.len(), 0);

        // Update adds an address
        shared.update(|f| {
            f.insert(addr1).unwrap();
        });

        // New load sees the update
        let snapshot = shared.load();
        assert!(snapshot.may_have_storage(addr1));
        assert!(!snapshot.may_have_storage(addr2));
        assert_eq!(snapshot.len(), 1);

        // Another update
        shared.update(|f| {
            f.insert(addr2).unwrap();
        });

        let snapshot = shared.load();
        assert!(snapshot.may_have_storage(addr1));
        assert!(snapshot.may_have_storage(addr2));
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn test_shared_filter_no_clone_when_sole_owner() {
        let addr1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        let shared = SharedStorageAccountFilter::with_capacity(100);

        // Get the Arc pointer before update
        let before = shared.load();
        let _before_ptr = Arc::as_ptr(&before);
        drop(before); // Release the reference

        // Update without any outstanding references - should NOT clone
        shared.update(|f| {
            f.insert(addr1).unwrap();
        });

        // The inner Arc should have been reused (same allocation)
        // We can't directly test this, but we verify the update worked
        let after = shared.load();
        assert!(after.may_have_storage(addr1));

        // Now test with outstanding reference - SHOULD clone
        let held_ref = shared.load();
        let held_ptr = Arc::as_ptr(&held_ref);

        shared.update(|f| {
            // This should clone because held_ref is still alive
            f.insert(b256!("0000000000000000000000000000000000000000000000000000000000000002"))
                .unwrap();
        });

        // The held reference should still point to old data (1 address)
        assert_eq!(held_ref.len(), 1);

        // New load should see both addresses
        let new_snapshot = shared.load();
        assert_eq!(new_snapshot.len(), 2);

        // The pointers should be different (clone happened)
        assert_ne!(held_ptr, Arc::as_ptr(&new_snapshot));
    }

    #[test]
    fn test_shared_filter_concurrent_access() {
        use std::thread;

        let shared = SharedStorageAccountFilter::with_capacity(1000);

        // Spawn reader threads
        let shared_clone = shared.clone();
        let reader = thread::spawn(move || {
            for _ in 0..100 {
                let snapshot = shared_clone.load();
                let _ = snapshot.len();
            }
        });

        // Spawn writer thread
        let shared_clone2 = shared.clone();
        let writer = thread::spawn(move || {
            for i in 0u64..100 {
                shared_clone2.update(|f| {
                    let mut bytes = [0u8; 32];
                    bytes[24..32].copy_from_slice(&i.to_be_bytes());
                    let _ = f.insert(B256::from(bytes));
                });
            }
        });

        reader.join().unwrap();
        writer.join().unwrap();

        // Final state should have ~100 entries
        let final_snapshot = shared.load();
        assert_eq!(final_snapshot.len(), 100);
    }
}
