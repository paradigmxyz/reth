//! Tracks state of RPC cache.

use metrics::Counter;
use reth_metrics::{metrics::Gauge, Metrics};

#[derive(Metrics)]
#[metrics(scope = "rpc.eth_cache")]
pub(crate) struct CacheMetrics {
    /// The number of entities in the cache.
    pub(crate) cached_count: Gauge,
    /// The number of queued consumers.
    pub(crate) queued_consumers_count: Gauge,
    /// The number of cache hits.
    pub(crate) hits_total: Counter,
    /// The number of cache misses.
    pub(crate) misses_total: Counter,
}
