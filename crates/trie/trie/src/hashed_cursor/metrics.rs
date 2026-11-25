use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::TrieType;
use alloy_primitives::B256;
use reth_metrics::metrics::{self, Histogram};
use reth_primitives_traits::FastInstant as Instant;
use reth_storage_errors::db::DatabaseError;
use std::time::Duration;

/// Prometheus metrics for hashed cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[derive(Clone, Debug)]
pub struct HashedCursorMetrics {
    /// Histogram tracking overall time spent in database operations, in seconds.
    overall_duration_seconds: Histogram,
    /// Histogram for `next()` operations
    next_histogram: Histogram,
    /// Histogram for `seek()` operations
    seek_histogram: Histogram,
    /// Histogram for `is_storage_empty()` operations
    is_storage_empty_histogram: Histogram,
}

impl HashedCursorMetrics {
    /// Create a new metrics instance with the specified trie type label.
    pub fn new(trie_type: TrieType) -> Self {
        let trie_type_str = trie_type.as_str();

        Self {
            overall_duration_seconds: metrics::histogram!(
                "trie.hashed_cursor.overall_duration_seconds",
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

    /// Wraps a cursor with owned counters that are recorded when the cursor is dropped.
    pub fn instrument<C>(&self, cursor: C) -> InstrumentedHashedCursor<C> {
        InstrumentedHashedCursor {
            cursor,
            metrics: self.clone(),
            next_count: 0,
            seek_count: 0,
            is_storage_empty_count: 0,
            total_duration: Duration::ZERO,
        }
    }
}

/// A wrapper around a [`HashedCursor`] that tracks metrics for cursor operations.
///
/// Counters are recorded into the supplied metric handles when the cursor is dropped.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
#[derive(Debug)]
pub struct InstrumentedHashedCursor<C> {
    /// The underlying cursor being wrapped.
    cursor: C,
    /// Metric handles used to record counters when the cursor is dropped.
    metrics: HashedCursorMetrics,
    /// Counter for `next()` calls.
    next_count: usize,
    /// Counter for `seek()` calls.
    seek_count: usize,
    /// Counter for `is_storage_empty` calls.
    is_storage_empty_count: usize,
    /// Total duration spent in cursor operations.
    total_duration: Duration,
}

impl<C> HashedCursor for InstrumentedHashedCursor<C>
where
    C: HashedCursor,
{
    type Value = C::Value;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.seek_count += 1;
        let result = self.cursor.seek(key);
        self.total_duration += start.elapsed();
        result
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.next_count += 1;
        let result = self.cursor.next();
        self.total_duration += start.elapsed();
        result
    }

    fn reset(&mut self) {
        self.cursor.reset()
    }
}

impl<C> HashedStorageCursor for InstrumentedHashedCursor<C>
where
    C: HashedStorageCursor,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let start = Instant::now();
        self.is_storage_empty_count += 1;
        let result = self.cursor.is_storage_empty();
        self.total_duration += start.elapsed();
        result
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address)
    }
}

impl<C> Drop for InstrumentedHashedCursor<C> {
    fn drop(&mut self) {
        self.metrics.next_histogram.record(self.next_count as f64);
        self.metrics.seek_histogram.record(self.seek_count as f64);
        self.metrics.is_storage_empty_histogram.record(self.is_storage_empty_count as f64);
        self.metrics.overall_duration_seconds.record(self.total_duration.as_secs_f64());
    }
}

/// Instruments every account and storage cursor opened by the wrapped factory.
///
/// Counters are owned by each cursor and recorded into the supplied handles on drop. Wrap the
/// outermost overlay factory to include both overlay and database operations in these counters.
#[derive(Clone, Debug)]
pub struct InstrumentedHashedCursorFactory<F> {
    factory: F,
    account_metrics: HashedCursorMetrics,
    storage_metrics: HashedCursorMetrics,
}

impl<F> InstrumentedHashedCursorFactory<F> {
    /// Wraps a factory with account and storage metric handles shared by its cursors.
    pub const fn new(
        factory: F,
        account_metrics: HashedCursorMetrics,
        storage_metrics: HashedCursorMetrics,
    ) -> Self {
        Self { factory, account_metrics, storage_metrics }
    }
}

impl<F: HashedCursorFactory> HashedCursorFactory for InstrumentedHashedCursorFactory<F> {
    type AccountCursor<'a>
        = InstrumentedHashedCursor<F::AccountCursor<'a>>
    where
        Self: 'a;
    type StorageCursor<'a>
        = InstrumentedHashedCursor<F::StorageCursor<'a>>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(self.account_metrics.instrument(self.factory.hashed_account_cursor()?))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(self.storage_metrics.instrument(self.factory.hashed_storage_cursor(hashed_address)?))
    }
}
