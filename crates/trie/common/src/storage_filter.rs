//! Cuckoo filter for tracking accounts with storage.
//!
//! This filter is used to skip storage proof calculations for accounts that
//! definitely have no storage, providing a significant performance optimization.

use alloy_primitives::B256;
use core::{fmt, hash::Hasher};
use cuckoofilter::{CuckooError, CuckooFilter};

/// Identity hasher for u64 keys - B256 is already hashed, no need to re-hash
#[derive(Default)]
struct U64IdentityHasher(u64);

impl Hasher for U64IdentityHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut acc = 0u64;
        for chunk in bytes.chunks(8) {
            let mut buf = [0u8; 8];
            buf[..chunk.len()].copy_from_slice(chunk);
            acc ^= u64::from_le_bytes(buf);
        }
        self.0 = acc;
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// XOR-fold B256 to u64. B256 is already uniformly distributed (it's a hash),
/// so we just need to reduce it to 64 bits for the cuckoo filter.
#[inline]
fn b256_to_u64(k: &B256) -> u64 {
    let b = k.as_slice();
    u64::from_le_bytes(b[0..8].try_into().unwrap())
        ^ u64::from_le_bytes(b[8..16].try_into().unwrap())
        ^ u64::from_le_bytes(b[16..24].try_into().unwrap())
        ^ u64::from_le_bytes(b[24..32].try_into().unwrap())
}

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
    filter: CuckooFilter<U64IdentityHasher>,
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
        self.filter.contains(&b256_to_u64(&hashed_address))
    }

    /// Inserts an account into the filter.
    ///
    /// Returns `Ok(())` if successful, or `Err(CuckooError::NotEnoughSpace)` if
    /// the filter is too full. When this error occurs, the element was still
    /// added but a random other element was evicted.
    #[inline]
    pub fn insert(&mut self, hashed_address: B256) -> Result<(), CuckooError> {
        self.filter.add(&b256_to_u64(&hashed_address))
    }

    /// Removes an account from the filter.
    ///
    /// Returns `true` if the account was present and removed, `false` otherwise.
    /// This should only be called when an account is destroyed and all its
    /// storage is cleared.
    #[inline]
    pub fn remove(&mut self, hashed_address: B256) -> bool {
        self.filter.delete(&b256_to_u64(&hashed_address))
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

impl Clone for StorageAccountFilter {
    fn clone(&self) -> Self {
        Self { filter: CuckooFilter::from(self.filter.export()) }
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

/// A lock-free wrapper around [`StorageAccountFilter`] using atomic swap semantics.
///
/// This type provides lock-free reads on the hot path (via [`Self::may_have_storage`]) while
/// allowing atomic updates from a background thread (via [`Self::swap`]).
///
/// The design uses `ArcSwap` for the inner filter, which provides:
/// - Lock-free reads that never block
/// - Atomic swaps for updates (writers build a new filter and swap it in)
/// - RCU-style (read-copy-update) semantics for safe concurrent access
pub struct SharedStorageFilter {
    inner: arc_swap::ArcSwap<StorageAccountFilter>,
}

impl fmt::Debug for SharedStorageFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedStorageFilter").field("inner", &*self.inner.load()).finish()
    }
}

impl SharedStorageFilter {
    /// Creates a new shared filter wrapping the given filter.
    pub fn new(filter: StorageAccountFilter) -> Self {
        Self { inner: arc_swap::ArcSwap::from_pointee(filter) }
    }

    /// Creates a new shared filter with the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(StorageAccountFilter::with_capacity(capacity))
    }

    /// Returns `true` if the account may have storage (lock-free read).
    ///
    /// This is the hot path - no locks are acquired.
    #[inline]
    pub fn may_have_storage(&self, hashed_address: B256) -> bool {
        self.inner.load().may_have_storage(hashed_address)
    }

    /// Atomically swaps the filter with a new one, returning the old filter.
    ///
    /// This is used by the update path to atomically replace the entire filter.
    pub fn swap(&self, new_filter: StorageAccountFilter) -> alloc::sync::Arc<StorageAccountFilter> {
        self.inner.swap(alloc::sync::Arc::new(new_filter))
    }

    /// Loads a reference to the current filter.
    ///
    /// Returns an `Arc` guard that keeps the filter alive while in use.
    #[inline]
    pub fn load(&self) -> arc_swap::Guard<alloc::sync::Arc<StorageAccountFilter>> {
        self.inner.load()
    }

    /// Returns the number of accounts tracked by the filter.
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.load().len()
    }

    /// Returns `true` if the filter is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.load().is_empty()
    }

    /// Returns the memory usage of the filter in bytes.
    #[inline]
    pub fn memory_usage(&self) -> usize {
        self.inner.load().memory_usage()
    }
}

impl Default for SharedStorageFilter {
    fn default() -> Self {
        Self::new(StorageAccountFilter::default())
    }
}

impl Clone for SharedStorageFilter {
    fn clone(&self) -> Self {
        let current = self.inner.load();
        Self { inner: arc_swap::ArcSwap::new(alloc::sync::Arc::clone(&*current)) }
    }
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
