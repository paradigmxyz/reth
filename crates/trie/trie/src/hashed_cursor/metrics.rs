use super::{HashedCursor, HashedStorageCursor};
use alloy_primitives::B256;
use reth_primitives_traits::FastInstant as Instant;
use reth_storage_errors::db::DatabaseError;
use std::time::Duration;

#[cfg(feature = "metrics")]
use super::HashedCursorFactory;
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
    /// Histogram tracking overall time spent in database operations, in seconds.
    overall_duration_seconds: Histogram,
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

    /// Record the cached metrics from the provided cache and reset the cache counters.
    ///
    /// This method adds the current counter values from the cache to the Prometheus metrics
    /// and then resets all cache counters to zero.
    pub fn record(&self, cache: &mut HashedCursorMetricsCache) {
        self.next_histogram.record(cache.next_count as f64);
        self.seek_histogram.record(cache.seek_count as f64);
        self.is_storage_empty_histogram.record(cache.is_storage_empty_count as f64);
        self.overall_duration_seconds.record(cache.total_duration.as_secs_f64());
        cache.reset();
    }

    /// Wraps a cursor with owned counters that are recorded when the cursor is dropped.
    pub fn instrument<C>(&self, cursor: C) -> InstrumentedHashedCursor<C> {
        InstrumentedHashedCursor { cursor, cache: Default::default(), metrics: Some(self.clone()) }
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
}

impl AsMut<Self> for HashedCursorMetricsCache {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

/// A wrapper around a [`HashedCursor`] that tracks metrics for cursor operations.
///
/// When constructed through `HashedCursorMetrics::instrument`, counters are recorded on drop.
/// The `new` constructor instead accumulates counters for the caller to record.
///
/// This implementation counts the number of times each cursor operation is called:
/// - `next()` - Move to the next entry
/// - `seek()` - Seek to a key or the next greater key
#[derive(Debug)]
pub struct InstrumentedHashedCursor<
    C,
    M: AsMut<HashedCursorMetricsCache> = HashedCursorMetricsCache,
> {
    /// The underlying cursor being wrapped
    cursor: C,
    /// Owned or borrowed counters for cursor operations.
    cache: M,
    /// Metric handles used to record owned counters when the cursor is dropped.
    #[cfg(feature = "metrics")]
    metrics: Option<HashedCursorMetrics>,
}

impl<C, M: AsMut<HashedCursorMetricsCache>> InstrumentedHashedCursor<C, M> {
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

impl<C, M: AsMut<HashedCursorMetricsCache>> HashedCursor for InstrumentedHashedCursor<C, M>
where
    C: HashedCursor,
{
    type Value = C::Value;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().seek_count += 1;
        let result = self.cursor.seek(key);
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().next_count += 1;
        let result = self.cursor.next();
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn reset(&mut self) {
        self.cursor.reset()
    }
}

impl<C, M: AsMut<HashedCursorMetricsCache>> HashedStorageCursor for InstrumentedHashedCursor<C, M>
where
    C: HashedStorageCursor,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let start = Instant::now();
        self.cache.as_mut().is_storage_empty_count += 1;
        let result = self.cursor.is_storage_empty();
        self.cache.as_mut().total_duration += start.elapsed();
        result
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address)
    }
}

#[cfg(feature = "metrics")]
impl<C, M: AsMut<HashedCursorMetricsCache>> Drop for InstrumentedHashedCursor<C, M> {
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
pub struct InstrumentedHashedCursorFactory<F> {
    factory: F,
    account_metrics: HashedCursorMetrics,
    storage_metrics: HashedCursorMetrics,
}

#[cfg(feature = "metrics")]
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

#[cfg(feature = "metrics")]
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

/// Uses the underlying factory directly when cursor metrics are disabled.
#[cfg(not(feature = "metrics"))]
pub type InstrumentedHashedCursorFactory<F> = F;
