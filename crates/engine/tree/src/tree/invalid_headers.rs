use alloy_eips::eip1898::BlockWithParent;
use alloy_primitives::B256;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use schnellru::{ByLength, LruMap};
use std::fmt::Debug;
use tracing::warn;

/// Keeps track of invalid headers.
#[derive(Debug)]
pub struct InvalidHeaderCache {
    /// This maps a header hash to a reference to its invalid ancestor.
    headers: LruMap<B256, HeaderEntry>,
    /// Number of cache hits before an invalid header entry is evicted and reprocessed.
    hit_eviction_threshold: u8,
    /// Metrics for the cache.
    metrics: InvalidHeaderCacheMetrics,
}

impl InvalidHeaderCache {
    /// Invalid header cache constructor.
    ///
    /// Setting `hit_eviction_threshold` to `0` effectively disables the cache because entries are
    /// evicted on the first lookup.
    pub fn new(max_length: u32, hit_eviction_threshold: u8) -> Self {
        Self {
            headers: LruMap::new(ByLength::new(max_length)),
            hit_eviction_threshold,
            metrics: Default::default(),
        }
    }

    fn insert_entry(&mut self, hash: B256, header: BlockWithParent) {
        self.headers.insert(hash, HeaderEntry { header, hit_count: 0 });
    }

    /// Returns the invalid ancestor's header if it exists in the cache.
    ///
    /// If this is called, the hit count for the entry is incremented.
    /// If the hit count exceeds the threshold, the entry is evicted and `None` is returned.
    pub fn get(&mut self, hash: &B256) -> Option<BlockWithParent> {
        {
            let entry = self.headers.get(hash)?;
            entry.hit_count += 1;
            if entry.hit_count < self.hit_eviction_threshold {
                return Some(entry.header)
            }
        }
        // if we get here, the entry has been hit too many times, so we evict it
        self.headers.remove(hash);
        self.metrics.hit_evictions.increment(1);
        self.metrics.count.set(self.headers.len() as f64);
        None
    }

    /// Inserts an invalid block into the cache, with a given invalid ancestor.
    pub fn insert_with_invalid_ancestor(
        &mut self,
        header_hash: B256,
        invalid_ancestor: BlockWithParent,
    ) {
        if self.get(&header_hash).is_none() {
            warn!(target: "consensus::engine", hash=?header_hash, ?invalid_ancestor, "Bad block with existing invalid ancestor");
            self.insert_entry(header_hash, invalid_ancestor);

            // update metrics
            self.metrics.known_ancestor_inserts.increment(1);
            self.metrics.count.set(self.headers.len() as f64);
        }
    }

    /// Inserts an invalid ancestor into the map.
    pub fn insert(&mut self, invalid_ancestor: BlockWithParent) {
        if self.get(&invalid_ancestor.block.hash).is_none() {
            warn!(target: "consensus::engine", ?invalid_ancestor, "Bad block with hash");
            self.insert_entry(invalid_ancestor.block.hash, invalid_ancestor);

            // update metrics
            self.metrics.unique_inserts.increment(1);
            self.metrics.count.set(self.headers.len() as f64);
        }
    }
}

struct HeaderEntry {
    /// Keeps track how many times this header has been hit.
    hit_count: u8,
    /// The actual header entry
    header: BlockWithParent,
}

/// Metrics for the invalid headers cache.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon.invalid_headers")]
struct InvalidHeaderCacheMetrics {
    /// The total number of invalid headers in the cache.
    count: Gauge,
    /// The number of inserts with a known ancestor.
    known_ancestor_inserts: Counter,
    /// The number of unique invalid header inserts (i.e. without a known ancestor).
    unique_inserts: Counter,
    /// The number of times a header was evicted from the cache because it was hit too many times.
    hit_evictions: Counter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_primitives_traits::SealedHeader;

    #[test]
    fn test_hit_eviction() {
        let hit_eviction_threshold = 3;
        let mut cache = InvalidHeaderCache::new(10, hit_eviction_threshold);
        let header = Header::default();
        let header = SealedHeader::seal_slow(header);
        cache.insert(header.block_with_parent());
        assert_eq!(cache.headers.get(&header.hash()).unwrap().hit_count, 0);

        for hit in 1..hit_eviction_threshold {
            assert!(cache.get(&header.hash()).is_some());
            assert_eq!(cache.headers.get(&header.hash()).unwrap().hit_count, hit);
        }

        assert!(cache.get(&header.hash()).is_none());
    }

    #[test]
    fn test_zero_hit_eviction_threshold_effectively_disables_cache() {
        let mut cache = InvalidHeaderCache::new(10, 0);
        let header = SealedHeader::seal_slow(Header::default());
        cache.insert(header.block_with_parent());

        assert!(cache.get(&header.hash()).is_none());
        assert_eq!(cache.headers.len(), 0);
    }
}
