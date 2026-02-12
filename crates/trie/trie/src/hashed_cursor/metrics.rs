use super::{HashedCursor, HashedStorageCursor};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use std::time::{Duration, Instant};
use tracing::trace_span;

#[cfg(feature = "metrics")]
use crate::TrieType;
#[cfg(feature = "metrics")]
use reth_metrics::metrics::{self, Histogram};

/// Prometheus metrics for hashed cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[cfg(feature = "metrics")]
#[derive(Clone, Debug)]
pub struct HashedCursorMetrics {
    /// Histogram tracking overall time spent in database operations
    overall_duration: Histogram,
    /// Histogram for `next()` operations
    next_histogram: Histogram,
    /// Histogram for `seek()` operations
    seek_histogram: Histogram,
    /// Histogram for `is_storage_empty()` operations
    is_storage_empty_histogram: Histogram,
}

#[cfg(feature = "metrics")]
impl HashedCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        let trie_type_str = trie_type.as_str();

        Self {
            overall_duration: metrics::histogram!(
                "trie.hashed_cursor.overall_duration",
                "type" => trie_type_str
            ),
            next_histogram: metrics::histogram!(
                "trie.hashed_cursor.operations",
                "type" => trie_type_str,
                "operation" => "next"
            ),
            seek_histogram: metrics::histogram!(
                "trie.hashed_cursor.operations",
                "type" => trie_type_str,
                "operation" => "seek"
            ),
            is_storage_empty_histogram: metrics::histogram!(
                "trie.hashed_cursor.operations",
                "type" => trie_type_str,
                "operation" => "is_storage_empty"
            ),
        }
    }

    /// Record the cached metrics from the provided cache and reset the cache counters.
    ///
    /// This method adds the current counter values from the cache to the Prometheus metrics
    /// and then resets all cache counters to zero.
    pub fn record(&mut self, cache: &mut HashedCursorMetricsCache) {
        self.next_histogram.record(cache.next_count as f64);
        self.seek_histogram.record(cache.seek_count as f64);
        self.is_storage_empty_histogram.record(cache.is_storage_empty_count as f64);
        self.overall_duration.record(cache.total_duration.as_secs_f64());
        cache.reset();
    }
}

/// Cached metrics counters for hashed cursor operations.
#[derive(Debug, Copy, Clone)]
pub struct HashedCursorMetricsCache {
    /// Counter for `next()` calls
    pub next_count: usize,
    /// Counter for `seek()` calls
    pub seek_count: usize,
    /// Counter for `is_storage_empty()` calls (if applicable)
    pub is_storage_empty_count: usize,
    /// Total duration spent in database operations
    pub total_duration: Duration,
}

impl Default for HashedCursorMetricsCache {
    fn default() -> Self {
        Self {
            next_count: 0,
            seek_count: 0,
            is_storage_empty_count: 0,
            total_duration: Duration::ZERO,
        }
    }
}

impl HashedCursorMetricsCache {
    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.next_count = 0;
        self.seek_count = 0;
        self.is_storage_empty_count = 0;
        self.total_duration = Duration::ZERO;
    }

    /// Extend this cache by adding the counts from another cache.
    ///
    /// This accumulates the counter values from `other` into this cache.
    pub fn extend(&mut self, other: &Self) {
        self.next_count += other.next_count;
        self.seek_count += other.seek_count;
        self.is_storage_empty_count += other.is_storage_empty_count;
        self.total_duration += other.total_duration;
    }

    /// Record the span for metrics.
    pub fn record_span(&self, name: &'static str) {
        let _span = trace_span!(
            target: "trie::trie_cursor",
            "Hashed cursor metrics",
            name,
            next_count = self.next_count,
            seek_count = self.seek_count,
            is_storage_empty_count = self.is_storage_empty_count,
            total_duration = self.total_duration.as_secs_f64(),
        )
        .entered();
    }
}

/// A wrapper around a [`HashedCursor`] that tracks metrics for cursor operations.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
#[derive(Debug)]
pub struct InstrumentedHashedCursor<'metrics, C> {
    /// The underlying cursor being wrapped
    cursor: C,
    /// Cached metrics counters
    metrics: &'metrics mut HashedCursorMetricsCache,
}

impl<'metrics, C> InstrumentedHashedCursor<'metrics, C> {
    /// Create a new metrics cursor wrapping the given cursor.
    pub const fn new(cursor: C, metrics: &'metrics mut HashedCursorMetricsCache) -> Self {
        Self { cursor, metrics }
    }
}

impl<'metrics, C> HashedCursor for InstrumentedHashedCursor<'metrics, C>
where
    C: HashedCursor,
{
    type Value = C::Value;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.metrics.seek_count += 1;
        let result = self.cursor.seek(key);
        self.metrics.total_duration += start.elapsed();
        result
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.metrics.next_count += 1;
        let result = self.cursor.next();
        self.metrics.total_duration += start.elapsed();
        result
    }

    fn reset(&mut self) {
        self.cursor.reset()
    }
}

impl<'metrics, C> HashedStorageCursor for InstrumentedHashedCursor<'metrics, C>
where
    C: HashedStorageCursor,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let start = Instant::now();
        self.metrics.is_storage_empty_count += 1;
        let result = self.cursor.is_storage_empty();
        self.metrics.total_duration += start.elapsed();
        result
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address)
    }
}
