use super::TrieCursor;
use crate::{BranchNodeCompact, Nibbles};
use reth_storage_errors::db::DatabaseError;

#[cfg(feature = "metrics")]
use crate::TrieType;
#[cfg(feature = "metrics")]
use reth_metrics::{metrics::Counter, Metrics};

/// Prometheus metrics for trie cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[cfg(feature = "metrics")]
#[derive(Metrics, Clone)]
#[metrics(scope = "trie.cursor")]
pub struct TrieCursorMetrics {
    /// Total number of `next()` calls
    next_total: Counter,
    /// Total number of `seek()` calls
    seek_total: Counter,
    /// Total number of `seek_exact()` calls
    seek_exact_total: Counter,
}

#[cfg(feature = "metrics")]
impl TrieCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        Self::new_with_labels(&[("type", trie_type.as_str())])
    }

    /// Record the cached metrics from the provided cache and reset the cache counters.
    ///
    /// This method adds the current counter values from the cache to the Prometheus metrics
    /// and then resets all cache counters to zero.
    pub fn record(&mut self, cache: &mut TrieCursorMetricsCache) {
        self.next_total.increment(cache.next_count as u64);
        self.seek_total.increment(cache.seek_count as u64);
        self.seek_exact_total.increment(cache.seek_exact_count as u64);
        cache.reset();
    }
}

/// Cached metrics counters for trie cursor operations.
#[derive(Debug, Default, Copy, Clone)]
pub struct TrieCursorMetricsCache {
    /// Counter for `next()` calls
    pub next_count: usize,
    /// Counter for `seek()` calls
    pub seek_count: usize,
    /// Counter for `seek_exact()` calls
    pub seek_exact_count: usize,
}

impl TrieCursorMetricsCache {
    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.next_count = 0;
        self.seek_count = 0;
        self.seek_exact_count = 0;
    }

    /// Extend this cache by adding the counts from another cache.
    ///
    /// This accumulates the counter values from `other` into this cache.
    pub const fn extend(&mut self, other: &Self) {
        self.next_count += other.next_count;
        self.seek_count += other.seek_count;
        self.seek_exact_count += other.seek_exact_count;
    }
}

/// A wrapper around a [`TrieCursor`] that tracks metrics for cursor operations.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
/// - `seek_exact()` - Seek to an exact key match
#[derive(Debug)]
pub struct InstrumentedTrieCursor<'metrics, C> {
    /// The underlying cursor being wrapped
    cursor: C,
    /// Cached metrics counters
    metrics: &'metrics mut TrieCursorMetricsCache,
}

impl<'metrics, C> InstrumentedTrieCursor<'metrics, C> {
    /// Create a new metrics cursor wrapping the given cursor.
    pub const fn new(cursor: C, metrics: &'metrics mut TrieCursorMetricsCache) -> Self {
        Self { cursor, metrics }
    }
}

impl<'metrics, C: TrieCursor> TrieCursor for InstrumentedTrieCursor<'metrics, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.metrics.seek_exact_count += 1;
        self.cursor.seek_exact(key)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.metrics.seek_count += 1;
        self.cursor.seek(key)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.metrics.next_count += 1;
        self.cursor.next()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.cursor.current()
    }
}
