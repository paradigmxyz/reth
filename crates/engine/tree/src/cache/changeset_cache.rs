//! In-memory cache for trie changesets.
//!
//! This module provides an efficient in-memory cache for trie changesets, which represent
//! the old values of trie nodes before a block was applied. The cache is essential for:
//!
//! - **Reorg support**: Quickly access changesets to revert blocks during chain reorganizations
//! - **Memory efficiency**: Automatic eviction ensures bounded memory usage
//!
//! ## Architecture
//!
//! The cache uses a dual data structure approach:
//! - `HashMap<B256, ...>` for fast O(1) lookups by block hash
//! - `BTreeMap<u64, ...>` for efficient range-based eviction by block number
//!
//! ## Eviction Policy
//!
//! The cache maintains a strict capacity limit (typically 64 blocks). When a new block
//! is inserted that exceeds capacity, the cache evicts the oldest blocks to maintain
//! the configured window size.
//!
//! Specifically, the cache keeps blocks in the range:
//! `[max_block_number - capacity, max_block_number]`
//!
//! ## Thread Safety
//!
//! The cache is designed to be wrapped in `Arc<RwLock<ChangesetCache>>` for concurrent
//! access. See [`ChangesetCacheHandle`] for the recommended usage pattern.

use alloy_primitives::B256;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use reth_trie_common::updates::TrieUpdatesSorted;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, RwLock},
};

/// Thread-safe handle to the changeset cache.
///
/// This type alias provides a convenient way to pass around a shared, mutable
/// reference to the changeset cache. The `RwLock` enables concurrent reads
/// while ensuring exclusive access for writes.
pub type ChangesetCacheHandle = Arc<RwLock<ChangesetCache>>;

/// In-memory cache for trie changesets with strict eviction policy.
///
/// Holds changesets for up to a configured number of blocks (typically 64),
/// evicting older blocks when the window moves forward. Keyed by block hash
/// for fast lookup during reorgs.
///
/// ## Capacity and Eviction
///
/// The cache maintains a sliding window of blocks. When `insert()` is called
/// with a block number higher than the current maximum, blocks outside the
/// retention window are automatically evicted.
///
/// For a cache with capacity 64:
/// - Block 100 is inserted → cache contains blocks 36-100 (if they exist)
/// - Block 101 is inserted → blocks 0-36 are evicted, cache contains 37-101
///
/// ## Metrics
///
/// The cache maintains several metrics for observability:
/// - `hits`: Number of successful cache lookups
/// - `misses`: Number of failed cache lookups
/// - `evictions`: Number of blocks evicted
/// - `size`: Current number of cached blocks
#[derive(Debug)]
pub struct ChangesetCache {
    /// Cache entries: block hash -> (block number, changesets)
    entries: HashMap<B256, (u64, Arc<TrieUpdatesSorted>)>,

    /// Block number to hashes mapping for eviction
    block_numbers: BTreeMap<u64, Vec<B256>>,

    /// Maximum number of blocks to cache
    ///
    /// This defines the size of the sliding window. Typically set to 64
    /// blocks, which covers about 2 epochs and handles most reorg scenarios.
    capacity: u64,

    /// Highest block number seen
    ///
    /// Used to determine the eviction window. When a new block with a higher
    /// number is inserted, this value is updated and blocks outside the
    /// retention window are evicted.
    max_block_number: Option<u64>,

    /// Metrics for monitoring cache behavior
    metrics: ChangesetCacheMetrics,
}

/// Metrics for the changeset cache.
///
/// These metrics provide visibility into cache performance and help identify
/// potential issues like high miss rates.
#[derive(Metrics, Clone)]
#[metrics(scope = "engine.changeset_cache")]
struct ChangesetCacheMetrics {
    /// Cache hit counter
    hits: Counter,

    /// Cache miss counter
    misses: Counter,

    /// Eviction counter
    evictions: Counter,

    /// Current cache size (number of entries)
    size: Gauge,
}

impl ChangesetCache {
    /// Creates a new cache with the specified capacity.
    pub fn new(capacity: u64) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity as usize),
            block_numbers: BTreeMap::new(),
            capacity,
            max_block_number: None,
            metrics: Default::default(),
        }
    }

    /// Retrieves changesets for a block by hash.
    ///
    /// Returns `None` if the block is not in the cache (either evicted or never computed).
    /// Updates hit/miss metrics accordingly.
    pub fn get(&self, block_hash: &B256) -> Option<Arc<TrieUpdatesSorted>> {
        match self.entries.get(block_hash) {
            Some((_, changesets)) => {
                self.metrics.hits.increment(1);
                Some(Arc::clone(changesets))
            }
            None => {
                self.metrics.misses.increment(1);
                None
            }
        }
    }

    /// Inserts changesets for a block, triggering eviction if needed.
    ///
    /// # Eviction Policy
    ///
    /// Keeps blocks in range `[max_block_number - capacity, max_block_number]`.
    /// When a new block with number N is inserted:
    /// - Update `max_block_number` if N > current max
    /// - Remove all blocks with number < (`max_block_number` - capacity)
    ///
    /// # Arguments
    ///
    /// * `block_hash` - Hash of the block
    /// * `block_number` - Block number for eviction tracking
    /// * `changesets` - Trie changesets to cache
    pub fn insert(
        &mut self,
        block_hash: B256,
        block_number: u64,
        changesets: Arc<TrieUpdatesSorted>,
    ) {
        // Check if this block is already outside the retention window
        if let Some(current_max) = self.max_block_number {
            let eviction_threshold = current_max.saturating_sub(self.capacity);
            if block_number < eviction_threshold {
                // Don't insert blocks that are already outside the window
                return;
            }
        }

        // Update max_block_number if this is a newer block
        let needs_eviction = if let Some(current_max) = self.max_block_number {
            if block_number > current_max {
                self.max_block_number = Some(block_number);
                true
            } else {
                false
            }
        } else {
            self.max_block_number = Some(block_number);
            false
        };

        // Insert the entry
        self.entries.insert(block_hash, (block_number, changesets));

        // Add block hash to block_numbers mapping
        self.block_numbers.entry(block_number).or_default().push(block_hash);

        // Trigger eviction if we have a new max block number
        if needs_eviction {
            self.evict();
        }

        // Update size metric
        self.metrics.size.set(self.entries.len() as f64);
    }

    /// Evicts blocks outside the retention window.
    ///
    /// This internal method is called by `insert()` when a new maximum block
    /// number is encountered. It removes all blocks with numbers less than
    /// `max_block_number - capacity`.
    ///
    /// # Implementation Details
    ///
    /// Uses the `block_numbers` `BTreeMap` to efficiently find and remove
    /// blocks by range. For each evicted block number:
    /// 1. Get all hashes at that block number
    /// 2. Remove entries from `entries` `HashMap`
    /// 3. Remove the block number from `block_numbers`
    fn evict(&mut self) {
        let Some(max_block) = self.max_block_number else {
            return;
        };

        // Calculate the eviction threshold
        // We keep blocks in range [max_block - capacity, max_block]
        let eviction_threshold = max_block.saturating_sub(self.capacity);

        // Find all block numbers that should be evicted
        let blocks_to_evict: Vec<u64> =
            self.block_numbers.range(..eviction_threshold).map(|(num, _)| *num).collect();

        // Track number of evicted entries for metrics
        let mut evicted_count = 0;

        // Remove entries for each block number below threshold
        for block_number in blocks_to_evict {
            if let Some(hashes) = self.block_numbers.remove(&block_number) {
                for hash in hashes {
                    if self.entries.remove(&hash).is_some() {
                        evicted_count += 1;
                    }
                }
            }
        }

        // Update metrics if we evicted anything
        if evicted_count > 0 {
            self.metrics.evictions.increment(evicted_count as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::B256Map;

    // Helper function to create empty TrieUpdatesSorted for testing
    fn create_test_changesets() -> Arc<TrieUpdatesSorted> {
        Arc::new(TrieUpdatesSorted::new(vec![], B256Map::default()))
    }

    #[test]
    fn test_insert_and_retrieve_single_entry() {
        let mut cache = ChangesetCache::new(64);
        let hash = B256::random();
        let changesets = create_test_changesets();

        cache.insert(hash, 100, Arc::clone(&changesets));

        // Should be able to retrieve it
        let retrieved = cache.get(&hash);
        assert!(retrieved.is_some());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.max_block_number, Some(100));
    }

    #[test]
    fn test_insert_multiple_entries() {
        let mut cache = ChangesetCache::new(64);

        // Insert 10 blocks
        let mut hashes = Vec::new();
        for i in 0..10 {
            let hash = B256::random();
            cache.insert(hash, 100 + i, create_test_changesets());
            hashes.push(hash);
        }

        // Should be able to retrieve all
        assert_eq!(cache.entries.len(), 10);
        for hash in &hashes {
            assert!(cache.get(hash).is_some());
        }
        assert_eq!(cache.max_block_number, Some(109));
    }

    #[test]
    fn test_eviction_when_capacity_exceeded() {
        let mut cache = ChangesetCache::new(10);

        // Insert 15 blocks (0-14)
        let mut hashes = Vec::new();
        for i in 0..15 {
            let hash = B256::random();
            cache.insert(hash, i, create_test_changesets());
            hashes.push((i, hash));
        }

        // Cache should have evicted blocks 0-4 (14 - 10 + 1 = 5)
        // Retention window: [14 - 10, 14] = [4, 14]
        // So blocks 0, 1, 2, 3 should be evicted
        assert_eq!(cache.entries.len(), 11); // blocks 4-14 = 11 blocks

        // Verify blocks 0-3 are evicted
        for i in 0..4 {
            assert!(cache.get(&hashes[i as usize].1).is_none(), "Block {} should be evicted", i);
        }

        // Verify blocks 4-14 are still present
        for i in 4..15 {
            assert!(cache.get(&hashes[i as usize].1).is_some(), "Block {} should be present", i);
        }
    }

    #[test]
    fn test_eviction_window_logic() {
        let mut cache = ChangesetCache::new(64);

        // Insert block 100
        let hash_100 = B256::random();
        cache.insert(hash_100, 100, create_test_changesets());
        assert!(cache.get(&hash_100).is_some());

        // Insert block 165 - should evict blocks < (165 - 64) = 101
        let hash_165 = B256::random();
        cache.insert(hash_165, 165, create_test_changesets());

        // Block 100 should be evicted
        assert!(cache.get(&hash_100).is_none());
        // Block 165 should be present
        assert!(cache.get(&hash_165).is_some());

        // Insert blocks 101-164
        for i in 101..165 {
            let hash = B256::random();
            cache.insert(hash, i, create_test_changesets());
        }

        // All blocks 101-165 should be present (65 blocks total)
        assert_eq!(cache.entries.len(), 65);
    }

    #[test]
    fn test_out_of_order_inserts() {
        let mut cache = ChangesetCache::new(10);

        // Insert blocks in random order
        let hash_10 = B256::random();
        cache.insert(hash_10, 10, create_test_changesets());

        let hash_5 = B256::random();
        cache.insert(hash_5, 5, create_test_changesets());

        let hash_15 = B256::random();
        cache.insert(hash_15, 15, create_test_changesets());

        let hash_3 = B256::random();
        cache.insert(hash_3, 3, create_test_changesets());

        // After inserting block 15, eviction threshold is 15 - 10 = 5
        // Blocks < 5 should be evicted
        assert!(cache.get(&hash_3).is_none(), "Block 3 should be evicted");
        assert!(cache.get(&hash_5).is_some(), "Block 5 should be present");
        assert!(cache.get(&hash_10).is_some(), "Block 10 should be present");
        assert!(cache.get(&hash_15).is_some(), "Block 15 should be present");
    }

    #[test]
    fn test_multiple_blocks_same_number() {
        let mut cache = ChangesetCache::new(64);

        // Insert multiple blocks with same number (side chains)
        let hash_1a = B256::random();
        let hash_1b = B256::random();
        cache.insert(hash_1a, 100, create_test_changesets());
        cache.insert(hash_1b, 100, create_test_changesets());

        // Both should be retrievable
        assert!(cache.get(&hash_1a).is_some());
        assert!(cache.get(&hash_1b).is_some());
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn test_eviction_with_side_chains() {
        let mut cache = ChangesetCache::new(5);

        // Insert blocks at same height (simulating side chains)
        let hash_1a = B256::random();
        let hash_1b = B256::random();
        cache.insert(hash_1a, 1, create_test_changesets());
        cache.insert(hash_1b, 1, create_test_changesets());

        // Insert block 7 - should evict blocks < (7 - 5) = 2
        let hash_7 = B256::random();
        cache.insert(hash_7, 7, create_test_changesets());

        // Both blocks at height 1 should be evicted
        assert!(cache.get(&hash_1a).is_none());
        assert!(cache.get(&hash_1b).is_none());
        assert!(cache.get(&hash_7).is_some());
    }
}
