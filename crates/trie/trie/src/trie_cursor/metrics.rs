use super::TrieCursor;
use crate::{BranchNodeCompact, Nibbles, TrieType};
use reth_metrics::{metrics::Counter, Metrics};
use reth_storage_errors::db::DatabaseError;

/// Prometheus metrics for trie cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
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

impl TrieCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        Self::new_with_labels(&[("type", trie_type.as_str())])
    }
}

/// Cached metrics counters for trie cursor operations.
#[derive(Debug, Default)]
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

    /// Record the cached metrics to the provided Prometheus metrics and reset counters.
    ///
    /// This method adds the current counter values to the Prometheus metrics
    /// and then resets all internal counters to zero.
    pub fn record(&mut self, metrics: &mut TrieCursorMetrics) {
        metrics.next_total.increment(self.next_count as u64);
        metrics.seek_total.increment(self.seek_count as u64);
        metrics.seek_exact_total.increment(self.seek_exact_count as u64);
        self.reset();
    }
}

/// A wrapper around a [`TrieCursor`] that tracks metrics for cursor operations.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
/// - `seek_exact()` - Seek to an exact key match
///
/// The counters can be reset using the [`reset`](TrieCursorMetricsCache::reset) method.
#[derive(Debug)]
pub struct InstrumentedTrieCursor<C> {
    /// The underlying cursor being wrapped
    cursor: C,
    /// Cached metrics counters
    pub metrics: TrieCursorMetricsCache,
}

impl<C> InstrumentedTrieCursor<C> {
    /// Create a new metrics cursor wrapping the given cursor.
    pub const fn new(cursor: C) -> Self {
        Self { cursor, metrics: Default::default() }
    }
}

impl<C: TrieCursor> TrieCursor for InstrumentedTrieCursor<C> {
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
