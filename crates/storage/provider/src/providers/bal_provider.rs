//! Block Access List (BAL) cache for EIP-7928.
//!
//! This module provides an in-memory cache for storing Block Access Lists received via
//! the Engine API. BALs are stored for valid payloads and can be retrieved through
//! Engine API BAL query paths that read from the cache/store provider.
//!
//! According to EIP-7928, the EL MUST retain BALs for at least the duration of the
//! weak subjectivity period (~3533 epochs) to support synchronization with re-execution.
//! This initial implementation uses a simple in-memory cache with configurable capacity.

use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_bal_store::{BalStore, BalStoreError};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tracing::warn;

/// Default capacity for the BAL cache.
///
/// This is a conservative default - production deployments should configure based on
/// weak subjectivity period requirements (~3533 epochs ≈ 113,000 blocks).
const DEFAULT_BAL_CACHE_CAPACITY: u32 = 1024;

/// In-memory cache for Block Access Lists (BALs).
///
/// Provides O(1) lookups by block hash and O(log n) range queries by block number.
/// Evicts the oldest (lowest) block numbers when capacity is exceeded.
///
/// This type is cheaply cloneable as it wraps an `Arc` internally.
#[derive(Debug, Clone)]
pub struct BalCache {
    inner: Arc<BalCacheInner>,
}

#[derive(Debug)]
struct BalCacheInner {
    /// Maximum number of entries to store.
    capacity: u32,
    /// Mapping from block hash to BAL bytes.
    entries: RwLock<HashMap<BlockHash, Bytes>>,
    /// Index mapping block number to block hash for range queries.
    /// Uses `BTreeMap` for efficient range iteration and eviction of oldest blocks.
    block_index: RwLock<BTreeMap<BlockNumber, BlockHash>>,
}

impl BalCache {
    /// Creates a new BAL cache with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BAL_CACHE_CAPACITY)
    }

    /// Creates a new BAL cache with the specified capacity.
    pub fn with_capacity(capacity: u32) -> Self {
        Self {
            inner: Arc::new(BalCacheInner {
                capacity,
                entries: RwLock::new(HashMap::new()),
                block_index: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    /// Inserts a BAL into the cache.
    ///
    /// If a different hash already exists for this block number (reorg), the old entry
    /// is removed first. If the cache is at capacity, the oldest block number is evicted.
    pub fn insert(&self, block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) {
        let mut entries = self.inner.entries.write();
        let mut block_index = self.inner.block_index.write();

        // If this block number already has a different hash, remove the old entry
        if let Some(old_hash) = block_index.get(&block_number) &&
            *old_hash != block_hash
        {
            entries.remove(old_hash);
        }

        // Evict oldest block if at capacity and this is a new entry
        if !entries.contains_key(&block_hash) &&
            entries.len() as u32 >= self.inner.capacity &&
            let Some((&oldest_num, &oldest_hash)) = block_index.first_key_value()
        {
            entries.remove(&oldest_hash);
            block_index.remove(&oldest_num);
        }

        entries.insert(block_hash, bal);
        block_index.insert(block_number, block_hash);
    }

    /// Retrieves BALs for the given block hashes.
    ///
    /// Returns a vector with the same length as `block_hashes`, where each element
    /// is `Some(bal)` if found or `None` if not in cache.
    pub fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> Vec<Option<Bytes>> {
        let entries = self.inner.entries.read();
        block_hashes.iter().map(|hash| entries.get(hash).cloned()).collect()
    }

    /// Retrieves BALs for a range of blocks starting at `start` for `count` blocks.
    ///
    /// Returns a vector of contiguous BALs in block number order, stopping at the first
    /// missing block. This ensures the caller knows the returned BALs correspond to
    /// blocks `[start, start + len)`.
    pub fn get_by_range(&self, start: BlockNumber, count: u64) -> Vec<Bytes> {
        let entries = self.inner.entries.read();
        let block_index = self.inner.block_index.read();

        let mut result = Vec::new();
        for block_num in start..start.saturating_add(count) {
            let Some(hash) = block_index.get(&block_num) else {
                break;
            };
            let Some(bal) = entries.get(hash) else {
                break;
            };
            result.push(bal.clone());
        }
        result
    }

    /// Returns the number of entries in the cache.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.entries.read().len()
    }
}

impl Default for BalCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BalStore for BalCache {
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError> {
        Self::insert(self, block_hash, block_number, bal);
        Ok(())
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> Result<Vec<Option<Bytes>>, BalStoreError> {
        Ok(Self::get_by_hashes(self, block_hashes))
    }

    fn get_by_range(&self, start: BlockNumber, count: u64) -> Result<Vec<Bytes>, BalStoreError> {
        Ok(Self::get_by_range(self, start, count))
    }
}

/// Provides access to Block Access Lists (BALs).
///
/// `BalProvider` acts as a thin abstraction over:
/// - a **durable store** (`BalStore`) which is the source of truth
/// - an **in-memory cache** (`BalCache`) for fast access.
///
/// Reads are cache-first with fallback to the store.
/// Writes are store-first to ensure durability before cache visibility.
#[derive(Clone, Debug)]
pub struct BalProvider {
    /// Persistent storage backend for BALs.
    store: Arc<dyn BalStore>,
    /// In-memory cache for recently accessed BALs.
    cache: BalCache,
}

impl Default for BalProvider {
    fn default() -> Self {
        let cache = BalCache::new();
        Self { store: Arc::new(BalCache::new()), cache }
    }
}

impl BalProvider {
    fn new(store: Arc<dyn BalStore>, cache: BalCache) -> Self {
        Self { store, cache }
    }

    const fn cache(&self) -> &BalCache {
        &self.cache
    }

    // Persist first: store is the source of truth. We only populate the in-memory cache if
    // durability succeeds, so cache visibility cannot outlive failed persistence.
    // `Bytes` is consumed by each insert call, so we clone once for store and move the original
    // into cache.
    fn cache_bal(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> Result<(), BalStoreError> {
        self.store.insert(block_hash, block_number, bal.clone())?;
        self.cache.insert(block_hash, block_number, bal);
        Ok(())
    }

    // Cache-first lookup: keep request order and fill only cache misses from durable storage.
    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> Vec<Option<Bytes>> {
        let mut results = self.cache.get_by_hashes(block_hashes);

        // Collect missing positions so store fallback can patch holes in-place.
        let mut missing_hashes = Vec::new();
        let mut missing_indices = Vec::new();
        for (idx, result) in results.iter().enumerate() {
            if result.is_none() {
                missing_indices.push(idx);
                missing_hashes.push(block_hashes[idx]);
            }
        }

        if missing_hashes.is_empty() {
            return results;
        }

        match self.store.get_by_hashes(&missing_hashes) {
            Ok(store_results) => {
                for (missing_idx, store_result) in
                    missing_indices.into_iter().zip(store_results.into_iter())
                {
                    if let Some(value) = store_result {
                        results[missing_idx] = Some(value);
                    }
                }
            }
            Err(err) => {
                warn!(target: "rpc::engine", ?err, "Failed to retrieve BALs by hash from BAL store");
            }
        }

        results
    }

    // Cache range reads are contiguous and stop at the first gap.
    // Only the missing suffix is queried from store to avoid re-reading cached prefix.
    fn get_by_range(&self, start: BlockNumber, count: u64) -> Vec<Bytes> {
        let mut cache_results = self.cache.get_by_range(start, count);
        if cache_results.len() as u64 == count {
            return cache_results;
        }

        let cached_len = cache_results.len() as u64;
        let missing_start = start.saturating_add(cached_len);
        let missing_count = count - cached_len;

        match self.store.get_by_range(missing_start, missing_count) {
            Ok(mut store_results) => {
                cache_results.append(&mut store_results);
                cache_results
            }
            Err(err) => {
                warn!(target: "rpc::engine", ?err, "Failed to retrieve BALs by range from BAL store");
                cache_results
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_insert_and_get_by_hash() {
        let cache = BalCache::with_capacity(10);

        let hash1 = B256::random();
        let hash2 = B256::random();
        let bal1 = Bytes::from_static(b"bal1");
        let bal2 = Bytes::from_static(b"bal2");

        cache.insert(hash1, 1, bal1.clone());
        cache.insert(hash2, 2, bal2.clone());

        let results = cache.get_by_hashes(&[hash1, hash2, B256::random()]);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], Some(bal1));
        assert_eq!(results[1], Some(bal2));
        assert_eq!(results[2], None);
    }

    #[test]
    fn test_get_by_range() {
        let cache = BalCache::with_capacity(10);

        for i in 1..=5 {
            let hash = B256::random();
            let bal = Bytes::from(format!("bal{i}").into_bytes());
            cache.insert(hash, i, bal);
        }

        let results = cache.get_by_range(2, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_get_by_range_stops_at_gap() {
        let cache = BalCache::with_capacity(10);

        for i in [1, 2, 4, 5] {
            let hash = B256::random();
            let bal = Bytes::from(format!("bal{i}").into_bytes());
            cache.insert(hash, i, bal);
        }

        let results = cache.get_by_range(1, 5);
        assert_eq!(results.len(), 2);

        let results = cache.get_by_range(4, 3);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_eviction_oldest_first() {
        let cache = BalCache::with_capacity(3);

        for i in [10, 20, 30] {
            let hash = B256::random();
            cache.insert(hash, i, Bytes::from_static(b"bal"));
        }
        assert_eq!(cache.len(), 3);

        let hash40 = B256::random();
        cache.insert(hash40, 40, Bytes::from_static(b"bal40"));
        assert_eq!(cache.len(), 3);

        let results = cache.get_by_range(10, 1);
        assert_eq!(results.len(), 0);

        let results = cache.get_by_range(20, 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_reorg_replaces_hash() {
        let cache = BalCache::with_capacity(10);

        let hash1 = B256::random();
        let hash2 = B256::random();
        let bal1 = Bytes::from_static(b"bal1");
        let bal2 = Bytes::from_static(b"bal2");

        cache.insert(hash1, 100, bal1.clone());
        assert_eq!(cache.get_by_hashes(&[hash1])[0], Some(bal1));

        cache.insert(hash2, 100, bal2.clone());

        assert_eq!(cache.get_by_hashes(&[hash1])[0], None);
        assert_eq!(cache.get_by_hashes(&[hash2])[0], Some(bal2));
        assert_eq!(cache.len(), 1);
    }
}
