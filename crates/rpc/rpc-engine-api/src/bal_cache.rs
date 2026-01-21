//! Block Access List (BAL) cache for EIP-7928.
//!
//! This module provides an in-memory cache for storing Block Access Lists received via
//! the Engine API. BALs are stored for valid payloads and can be retrieved via
//! `engine_getBALsByHashV1` and `engine_getBALsByRangeV1`.
//!
//! According to EIP-7928, the EL MUST retain BALs for at least the duration of the
//! weak subjectivity period (~3533 epochs) to support synchronization with re-execution.
//! This initial implementation uses a simple in-memory LRU cache with configurable capacity.

use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use schnellru::{ByLength, LruMap};
use std::{collections::BTreeMap, sync::Arc};

/// Default capacity for the BAL cache.
///
/// This is a conservative default - production deployments should configure based on
/// weak subjectivity period requirements (~3533 epochs ≈ 113,000 blocks).
const DEFAULT_BAL_CACHE_CAPACITY: u32 = 1024;

/// Cache entry storing a BAL with its block number for range queries.
#[derive(Debug, Clone)]
struct BalEntry {
    /// The block number for range-based lookups.
    block_number: BlockNumber,
    /// The RLP-encoded block access list.
    bal: Bytes,
}

/// In-memory cache for Block Access Lists (BALs).
///
/// Provides O(1) lookups by block hash and O(log n) range queries by block number.
/// Uses an LRU eviction policy when the cache exceeds capacity.
///
/// This type is cheaply cloneable as it wraps an `Arc` internally.
#[derive(Debug, Clone)]
pub struct BalCache {
    inner: Arc<BalCacheInner>,
}

#[derive(Debug)]
struct BalCacheInner {
    /// LRU cache mapping block hash to BAL entry.
    entries: RwLock<LruMap<BlockHash, BalEntry>>,
    /// Index mapping block number to block hash for range queries.
    /// Uses `BTreeMap` for efficient range iteration.
    block_index: RwLock<BTreeMap<BlockNumber, BlockHash>>,
    /// Cache metrics.
    metrics: BalCacheMetrics,
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
                entries: RwLock::new(LruMap::new(ByLength::new(capacity))),
                block_index: RwLock::new(BTreeMap::new()),
                metrics: BalCacheMetrics::default(),
            }),
        }
    }

    /// Inserts a BAL into the cache.
    ///
    /// If the cache is at capacity, the least recently used entry will be evicted.
    pub fn insert(&self, block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) {
        let entry = BalEntry { block_number, bal };

        let mut entries = self.inner.entries.write();
        let mut block_index = self.inner.block_index.write();

        // Check if we need to evict an old entry
        if entries.len() as u32 >= entries.limiter().max_length() {
            // Find and remove the oldest entry from block_index
            if let Some((_, evicted_entry)) = entries.iter().next_back() {
                block_index.remove(&evicted_entry.block_number);
            }
        }

        entries.insert(block_hash, entry);
        block_index.insert(block_number, block_hash);

        self.inner.metrics.inserts.increment(1);
        self.inner.metrics.count.set(entries.len() as f64);
    }

    /// Retrieves BALs for the given block hashes.
    ///
    /// Returns a vector with the same length as `block_hashes`, where each element
    /// is `Some(bal)` if found or `None` if not in cache.
    pub fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> Vec<Option<Bytes>> {
        let mut entries = self.inner.entries.write();
        block_hashes
            .iter()
            .map(|hash| {
                let result = entries.get(hash).map(|e| e.bal.clone());
                if result.is_some() {
                    self.inner.metrics.hits.increment(1);
                } else {
                    self.inner.metrics.misses.increment(1);
                }
                result
            })
            .collect()
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
            let Some(entry) = entries.peek(hash) else {
                break;
            };
            result.push(entry.bal.clone());
        }
        result
    }

    /// Returns the number of entries in the cache.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.entries.read().len()
    }
}

impl Default for BalCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics for the BAL cache.
#[derive(Metrics)]
#[metrics(scope = "engine.bal_cache")]
struct BalCacheMetrics {
    /// The total number of BALs in the cache.
    count: Gauge,
    /// The number of cache inserts.
    inserts: Counter,
    /// The number of cache hits.
    hits: Counter,
    /// The number of cache misses.
    misses: Counter,
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

        // Insert blocks 1, 2, 4, 5 (missing block 3)
        for i in [1, 2, 4, 5] {
            let hash = B256::random();
            let bal = Bytes::from(format!("bal{i}").into_bytes());
            cache.insert(hash, i, bal);
        }

        // Requesting range starting at 1 should stop at the gap (block 3)
        let results = cache.get_by_range(1, 5);
        assert_eq!(results.len(), 2); // Only blocks 1 and 2

        // Requesting range starting at 4 should return 4 and 5
        let results = cache.get_by_range(4, 3);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_lru_eviction() {
        let cache = BalCache::with_capacity(3);

        let hashes: Vec<_> = (0..5).map(|_| B256::random()).collect();
        for (i, hash) in hashes.iter().enumerate() {
            cache.insert(*hash, i as u64, Bytes::from_static(b"bal"));
        }

        // Only the last 3 should be in cache
        assert_eq!(cache.len(), 3);

        // First two should be evicted
        let results = cache.get_by_hashes(&hashes[..2]);
        assert!(results.iter().all(|r| r.is_none()));

        // Last three should still be there
        let results = cache.get_by_hashes(&hashes[2..]);
        assert!(results.iter().all(|r| r.is_some()));
    }
}
