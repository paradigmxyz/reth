use std::sync::Arc;

use reth_metrics::{
    metrics::{self, Counter, Gauge},
    Metrics,
};
use reth_primitives::{Header, SealedHeader, H256};
use schnellru::{ByLength, LruMap};
use tracing::warn;

/// Metrics for the invalid headers cache.
#[derive(Metrics)]
#[metrics(scope = "invalid_header_cache")]
struct InvalidHeaderCacheMetrics {
    /// The total number of invalid headers in the cache.
    invalid_headers: Gauge,
    /// The number of inserts with a known ancestor.
    known_ancestor_inserts: Counter,
    /// The number of unique invalid header inserts (i.e. without a known ancestor).
    unique_inserts: Counter,
}

/// Keeps track of invalid headers.
pub(crate) struct InvalidHeaderCache {
    /// This maps a header hash to a reference to its invalid ancestor.
    headers: LruMap<H256, Arc<Header>>,
    /// Metrics for the cache.
    metrics: InvalidHeaderCacheMetrics,
}

impl InvalidHeaderCache {
    pub(crate) fn new(max_length: u32) -> Self {
        Self { headers: LruMap::new(ByLength::new(max_length)), metrics: Default::default() }
    }

    /// Returns the invalid ancestor's header if it exists in the cache.
    pub(crate) fn get(&mut self, hash: &H256) -> Option<&mut Arc<Header>> {
        self.headers.get(hash)
    }

    /// Inserts an invalid block into the cache, with a given invalid ancestor.
    pub(crate) fn insert_with_invalid_ancestor(
        &mut self,
        header_hash: H256,
        invalid_ancestor: Arc<Header>,
    ) {
        if self.headers.get(&header_hash).is_none() {
            warn!(target: "consensus::engine", hash=?header_hash, ?invalid_ancestor, "Bad block with existing invalid ancestor");
            self.headers.insert(header_hash, invalid_ancestor);

            // update metrics
            self.metrics.known_ancestor_inserts.increment(1);
            self.metrics.invalid_headers.set(self.headers.len() as f64);
        }
    }

    /// Inserts an invalid ancestor into the map.
    pub(crate) fn insert(&mut self, invalid_ancestor: SealedHeader) {
        if self.headers.get(&invalid_ancestor.hash).is_none() {
            let hash = invalid_ancestor.hash;
            let header = invalid_ancestor.unseal();
            warn!(target: "consensus::engine", ?hash, ?header, "Bad block with hash");
            self.headers.insert(hash, Arc::new(header));

            // update metrics
            self.metrics.unique_inserts.increment(1);
            self.metrics.invalid_headers.set(self.headers.len() as f64);
        }
    }
}
