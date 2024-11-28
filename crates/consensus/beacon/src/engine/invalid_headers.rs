use alloy_consensus::Header;
use alloy_primitives::B256;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use reth_primitives::SealedHeader;
use schnellru::{ByLength, LruMap};
use std::{fmt::Debug, sync::Arc};
use tracing::warn;

/// The max hit counter for invalid headers in the cache before it is forcefully evicted.
///
/// In other words, if a header is referenced more than this number of times, it will be evicted to
/// allow for reprocessing.
const INVALID_HEADER_HIT_EVICTION_THRESHOLD: u8 = 128;

/// Keeps track of invalid headers.
#[derive(Debug)]
pub struct InvalidHeaderCache<H = Header> {
    /// This maps a header hash to a reference to its invalid ancestor.
    headers: LruMap<B256, HeaderEntry<H>>,
    /// Metrics for the cache.
    metrics: InvalidHeaderCacheMetrics,
}

impl<H: Debug> InvalidHeaderCache<H> {
    /// Invalid header cache constructor.
    pub fn new(max_length: u32) -> Self {
        Self { headers: LruMap::new(ByLength::new(max_length)), metrics: Default::default() }
    }

    fn insert_entry(&mut self, hash: B256, header: Arc<H>) {
        self.headers.insert(hash, HeaderEntry { header, hit_count: 0 });
    }

    /// Returns the invalid ancestor's header if it exists in the cache.
    ///
    /// If this is called, the hit count for the entry is incremented.
    /// If the hit count exceeds the threshold, the entry is evicted and `None` is returned.
    pub fn get(&mut self, hash: &B256) -> Option<Arc<H>> {
        {
            let entry = self.headers.get(hash)?;
            entry.hit_count += 1;
            if entry.hit_count < INVALID_HEADER_HIT_EVICTION_THRESHOLD {
                return Some(entry.header.clone())
            }
        }
        // if we get here, the entry has been hit too many times, so we evict it
        self.headers.remove(hash);
        self.metrics.hit_evictions.increment(1);
        None
    }

    /// Inserts an invalid block into the cache, with a given invalid ancestor.
    pub fn insert_with_invalid_ancestor(&mut self, header_hash: B256, invalid_ancestor: Arc<H>) {
        if self.get(&header_hash).is_none() {
            warn!(target: "consensus::engine", hash=?header_hash, ?invalid_ancestor, "Bad block with existing invalid ancestor");
            self.insert_entry(header_hash, invalid_ancestor);

            // update metrics
            self.metrics.known_ancestor_inserts.increment(1);
            self.metrics.count.set(self.headers.len() as f64);
        }
    }

    /// Inserts an invalid ancestor into the map.
    pub fn insert(&mut self, invalid_ancestor: SealedHeader<H>) {
        if self.get(&invalid_ancestor.hash()).is_none() {
            let hash = invalid_ancestor.hash();
            let header = invalid_ancestor.unseal();
            warn!(target: "consensus::engine", ?hash, ?header, "Bad block with hash");
            self.insert_entry(hash, Arc::new(header));

            // update metrics
            self.metrics.unique_inserts.increment(1);
            self.metrics.count.set(self.headers.len() as f64);
        }
    }
}

struct HeaderEntry<H> {
    /// Keeps track how many times this header has been hit.
    hit_count: u8,
    /// The actually header entry
    header: Arc<H>,
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

    #[test]
    fn test_hit_eviction() {
        let mut cache = InvalidHeaderCache::new(10);
        let header = Header::default();
        let header = SealedHeader::seal(header);
        cache.insert(header.clone());
        assert_eq!(cache.headers.get(&header.hash()).unwrap().hit_count, 0);

        for hit in 1..INVALID_HEADER_HIT_EVICTION_THRESHOLD {
            assert!(cache.get(&header.hash()).is_some());
            assert_eq!(cache.headers.get(&header.hash()).unwrap().hit_count, hit);
        }

        assert!(cache.get(&header.hash()).is_none());
    }
}
