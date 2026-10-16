use super::{TrieCursor, TrieStorageCursor};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_primitives_traits::FastInstant as Instant;
use reth_storage_errors::db::DatabaseError;
use std::time::Duration;

#[cfg(feature = "metrics")]
use super::TrieCursorFactory;
#[cfg(feature = "metrics")]
use crate::TrieType;
#[cfg(feature = "metrics")]
use reth_metrics::metrics::{self, Histogram};

/// Prometheus metrics for trie cursor operations.
///
/// Tracks the number of cursor operations for monitoring and performance analysis.
#[cfg(feature = "metrics")]
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

#[cfg(feature = "metrics")]
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

    /// Record the cached metrics from the provided cache and reset the cache counters.
    ///
    /// This method adds the current counter values from the cache to the Prometheus metrics
    /// and then resets all cache counters to zero.
    pub fn record(&self, cache: &mut TrieCursorMetricsCache) {
        self.next_histogram.record(cache.next_count as f64);
        self.seek_histogram.record(cache.seek_count as f64);
        self.seek_exact_histogram.record(cache.seek_exact_count as f64);
        self.overall_duration_seconds.record(cache.total_duration.as_secs_f64());
        cache.reset();
    }

    /// Wraps a cursor with owned counters that are recorded when the cursor is dropped.
    pub fn instrument<C>(&self, cursor: C) -> InstrumentedTrieCursor<C> {
        InstrumentedTrieCursor { cursor, cache: Default::default(), metrics: Some(self.clone()) }
    }
}

/// Cached metrics counters for trie cursor operations.
#[derive(Debug, Copy, Clone)]
pub struct TrieCursorMetricsCache {
    /// Counter for `next()` calls
    pub next_count: usize,
    /// Counter for `seek()` calls
    pub seek_count: usize,
    /// Counter for `seek_exact()` calls
    pub seek_exact_count: usize,
    /// Total duration spent in database operations
    pub total_duration: Duration,
}

impl Default for TrieCursorMetricsCache {
    fn default() -> Self {
        Self { next_count: 0, seek_count: 0, seek_exact_count: 0, total_duration: Duration::ZERO }
    }
}

impl TrieCursorMetricsCache {
    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.next_count = 0;
        self.seek_count = 0;
        self.seek_exact_count = 0;
        self.total_duration = Duration::ZERO;
    }
}

impl AsMut<Self> for TrieCursorMetricsCache {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

/// A wrapper around a [`TrieCursor`] that tracks metrics for cursor operations.
///
/// When constructed through `TrieCursorMetrics::instrument`, counters are recorded on drop.
/// The `new` constructor instead accumulates counters for the caller to record.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
/// - `seek_exact()` - Seek to an exact key match
#[derive(Debug)]
pub struct InstrumentedTrieCursor<C, M: AsMut<TrieCursorMetricsCache> = TrieCursorMetricsCache> {
    /// The underlying cursor being wrapped
    cursor: C,
    /// Owned or borrowed counters for cursor operations.
    cache: M,
    /// Metric handles used to record owned counters when the cursor is dropped.
    #[cfg(feature = "metrics")]
    metrics: Option<TrieCursorMetrics>,
}

impl<C, M: AsMut<TrieCursorMetricsCache>> InstrumentedTrieCursor<C, M> {
    /// Wraps a cursor with owned or borrowed metrics counters.
    ///
    /// Counters accumulate in `cache`; recording them is the caller's responsibility.
    pub const fn new(cursor: C, cache: M) -> Self {
        Self {
            cursor,
            cache,
            #[cfg(feature = "metrics")]
            metrics: None,
        }
    }
}

impl<C: TrieCursor, M: AsMut<TrieCursorMetricsCache>> TrieCursor for InstrumentedTrieCursor<C, M> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().seek_exact_count += 1;
        let result = self.cursor.seek_exact(key);
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().seek_count += 1;
        let result = self.cursor.seek(key);
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().next_count += 1;
        let result = self.cursor.next();
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.cursor.current()
    }

    fn reset(&mut self) {
        self.cursor.reset()
    }
}

impl<C: TrieStorageCursor, M: AsMut<TrieCursorMetricsCache>> TrieStorageCursor
    for InstrumentedTrieCursor<C, M>
{
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address)
    }
}

#[cfg(feature = "metrics")]
impl<C, M: AsMut<TrieCursorMetricsCache>> Drop for InstrumentedTrieCursor<C, M> {
    fn drop(&mut self) {
        if let Some(metrics) = &self.metrics {
            metrics.record(self.cache.as_mut());
        }
    }
}

/// Instruments every account and storage cursor opened by the wrapped factory.
///
/// Counters are owned by each cursor and recorded into the supplied handles on drop. Wrap the
/// outermost overlay factory to include both overlay and database operations in these counters.
#[cfg(feature = "metrics")]
#[derive(Clone, Debug)]
pub struct InstrumentedTrieCursorFactory<F> {
    factory: F,
    account_metrics: TrieCursorMetrics,
    storage_metrics: TrieCursorMetrics,
}

#[cfg(feature = "metrics")]
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

#[cfg(feature = "metrics")]
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

/// Uses the underlying factory directly when cursor metrics are disabled.
#[cfg(not(feature = "metrics"))]
pub type InstrumentedTrieCursorFactory<F> = F;
