//! Cuckoo filter for tracking accounts with storage.
//!
//! This filter is used to skip storage proof calculations for accounts that
//! definitely have no storage, providing a significant performance optimization.

use alloy_primitives::{map::FbHasher, B256};
use cuckoofilter::{CuckooError, CuckooFilter};
use std::fmt;

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
    filter: CuckooFilter<FbHasher<20>>,
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

    /// Removes an account from the filter.
    ///
    /// Returns `true` if the account was present and removed, `false` otherwise.
    /// This should only be called when an account is destroyed and all its
    /// storage is cleared.
    #[inline]
    pub fn remove(&mut self, hashed_address: B256) -> bool {
        self.filter.delete(&hashed_address)
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

/// Update result from a bundle state update.
#[derive(Debug, Default)]
pub struct StorageFilterUpdateStats {
    /// Number of accounts inserted (had new storage).
    pub inserted: usize,
    /// Number of accounts removed (were destroyed).
    pub removed: usize,
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
    fn test_remove() {
        let mut filter = StorageAccountFilter::with_capacity(100);

        let addr = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        filter.insert(addr).unwrap();
        assert!(filter.may_have_storage(addr));

        let removed = filter.remove(addr);
        assert!(removed);
        assert!(!filter.may_have_storage(addr));
        assert_eq!(filter.len(), 0);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut filter = StorageAccountFilter::with_capacity(100);

        let addr = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        let removed = filter.remove(addr);
        assert!(!removed);
    }
}
