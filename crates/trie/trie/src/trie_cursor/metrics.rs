use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use crate::{BranchNodeCompact, Nibbles, TrieType};
use alloy_primitives::B256;
use reth_metrics::metrics::{self, Histogram};
use reth_primitives_traits::FastInstant as Instant;
use reth_storage_errors::db::DatabaseError;
use std::time::Duration;

/// Prometheus metrics for trie cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[derive(Clone, Debug)]
pub struct TrieCursorMetrics {
    /// Histogram tracking overall time spent in database operations, in seconds.
    overall_duration_seconds: Histogram,
    /// Histogram for `next()` operations
    next_histogram: Histogram,
    /// Histogram for `seek()` operations
    seek_histogram: Histogram,
    /// Histogram for `seek_exact()` operations
    seek_exact_histogram: Histogram,
}

impl TrieCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        let trie_type_str = trie_type.as_str();

        Self {
            overall_duration_seconds: metrics::histogram!(
                "trie.cursor.overall_duration_seconds",
                "type" => trie_type_str
            ),
            next_histogram: metrics::histogram!(
                "trie.cursor.operations",
                "type" => trie_type_str,
                "operation" => "next"
            ),
            seek_histogram: metrics::histogram!(
                "trie.cursor.operations",
                "type" => trie_type_str,
                "operation" => "seek"
            ),
            seek_exact_histogram: metrics::histogram!(
                "trie.cursor.operations",
                "type" => trie_type_str,
                "operation" => "seek_exact"
            ),
        }
    }

    /// Wraps a cursor with owned counters that are recorded when the cursor is dropped.
    pub fn instrument<C>(&self, cursor: C) -> InstrumentedTrieCursor<C> {
        InstrumentedTrieCursor {
            cursor,
            metrics: self.clone(),
            next_count: 0,
            seek_count: 0,
            seek_exact_count: 0,
            total_duration: Duration::ZERO,
        }
    }
}

/// A wrapper around a [`TrieCursor`] that tracks metrics for cursor operations.
///
/// Counters are recorded into the supplied metric handles when the cursor is dropped.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
/// - `seek_exact()` - Seek to an exact key match
#[derive(Debug)]
pub struct InstrumentedTrieCursor<C> {
    /// The underlying cursor being wrapped.
    cursor: C,
    /// Metric handles used to record counters when the cursor is dropped.
    metrics: TrieCursorMetrics,
    /// Counter for `next()` calls.
    next_count: usize,
    /// Counter for `seek()` calls.
    seek_count: usize,
    /// Counter for `seek_exact` calls.
    seek_exact_count: usize,
    /// Total duration spent in cursor operations.
    total_duration: Duration,
}

impl<C: TrieCursor> TrieCursor for InstrumentedTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.seek_exact_count += 1;
        let result = self.cursor.seek_exact(key);
        self.total_duration += start.elapsed();
        result
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.seek_count += 1;
        let result = self.cursor.seek(key);
        self.total_duration += start.elapsed();
        result
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.next_count += 1;
        let result = self.cursor.next();
        self.total_duration += start.elapsed();
        result
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.cursor.current()
    }

    fn reset(&mut self) {
        self.cursor.reset()
    }
}

impl<C: TrieStorageCursor> TrieStorageCursor for InstrumentedTrieCursor<C> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address)
    }
}

impl<C> Drop for InstrumentedTrieCursor<C> {
    fn drop(&mut self) {
        self.metrics.next_histogram.record(self.next_count as f64);
        self.metrics.seek_histogram.record(self.seek_count as f64);
        self.metrics.seek_exact_histogram.record(self.seek_exact_count as f64);
        self.metrics.overall_duration_seconds.record(self.total_duration.as_secs_f64());
    }
}

/// Instruments every account and storage cursor opened by the wrapped factory.
///
/// Counters are owned by each cursor and recorded into the supplied handles on drop. Wrap the
/// outermost overlay factory to include both overlay and database operations in these counters.
#[derive(Clone, Debug)]
pub struct InstrumentedTrieCursorFactory<F> {
    factory: F,
    account_metrics: TrieCursorMetrics,
    storage_metrics: TrieCursorMetrics,
}

impl<F> InstrumentedTrieCursorFactory<F> {
    /// Wraps a factory with account and storage metric handles shared by its cursors.
    pub const fn new(
        factory: F,
        account_metrics: TrieCursorMetrics,
        storage_metrics: TrieCursorMetrics,
    ) -> Self {
        Self { factory, account_metrics, storage_metrics }
    }
}

impl<F: TrieCursorFactory> TrieCursorFactory for InstrumentedTrieCursorFactory<F> {
    type AccountTrieCursor<'a>
        = InstrumentedTrieCursor<F::AccountTrieCursor<'a>>
    where
        Self: 'a;
    type StorageTrieCursor<'a>
        = InstrumentedTrieCursor<F::StorageTrieCursor<'a>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(self.account_metrics.instrument(self.factory.account_trie_cursor()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(self.storage_metrics.instrument(self.factory.storage_trie_cursor(hashed_address)?))
    }
}
