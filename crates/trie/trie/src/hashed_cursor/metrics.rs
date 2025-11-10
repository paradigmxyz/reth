use super::{HashedCursor, HashedStorageCursor};
use crate::TrieType;
use alloy_primitives::B256;
use reth_metrics::{metrics::Counter, Metrics};
use reth_storage_errors::db::DatabaseError;

/// Prometheus metrics for hashed cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[derive(Metrics, Clone)]
#[metrics(scope = "trie.hashed_cursor")]
pub struct HashedCursorMetrics {
    /// Total number of `next()` calls
    next_total: Counter,
    /// Total number of `seek()` calls
    seek_total: Counter,
    /// Total number of `is_storage_empty()` calls
    is_storage_empty_total: Counter,
}

impl HashedCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        Self::new_with_labels(&[("type", trie_type.as_str())])
    }
}

/// Cached metrics counters for hashed cursor operations.
#[derive(Debug, Default)]
pub struct HashedCursorMetricsCache {
    /// Counter for `next()` calls
    pub next_count: usize,
    /// Counter for `seek()` calls
    pub seek_count: usize,
    /// Counter for `is_storage_empty()` calls (if applicable)
    pub is_storage_empty_count: usize,
}

impl HashedCursorMetricsCache {
    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.next_count = 0;
        self.seek_count = 0;
        self.is_storage_empty_count = 0;
    }

    /// Extend this cache by adding the counts from another cache.
    ///
    /// This accumulates the counter values from `other` into this cache.
    pub const fn extend(&mut self, other: &Self) {
        self.next_count += other.next_count;
        self.seek_count += other.seek_count;
        self.is_storage_empty_count += other.is_storage_empty_count;
    }

    /// Record the cached metrics to the provided Prometheus metrics and reset counters.
    ///
    /// This method adds the current counter values to the Prometheus metrics
    /// and then resets all internal counters to zero.
    pub fn record(&mut self, metrics: &mut HashedCursorMetrics) {
        metrics.next_total.increment(self.next_count as u64);
        metrics.seek_total.increment(self.seek_count as u64);
        metrics.is_storage_empty_total.increment(self.is_storage_empty_count as u64);
        self.reset();
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
        self.metrics.seek_count += 1;
        self.cursor.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.metrics.next_count += 1;
        self.cursor.next()
    }
}

impl<'metrics, C> HashedStorageCursor for InstrumentedHashedCursor<'metrics, C>
where
    C: HashedStorageCursor,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.metrics.is_storage_empty_count += 1;
        self.cursor.is_storage_empty()
    }
}
