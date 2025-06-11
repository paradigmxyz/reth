use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::{map::B256Map, B256};
use mini_moka::sync::Cache;
use parking_lot::RwLock;
use reth_storage_errors::db::DatabaseError;
use std::sync::Arc;
use tracing::debug;

/// Default cache size for account trie operations.
const DEFAULT_ACCOUNT_CACHE_SIZE: u64 = 10_000;

/// Default cache size for storage trie operations.
const DEFAULT_STORAGE_CACHE_SIZE: u64 = 1_000;

/// Cache key for trie cursor operations.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum CacheKey {
    /// Seek exact operation with the given key.
    SeekExact(Nibbles),
    /// Seek operation with the given key.
    Seek(Nibbles),
    /// Next operation with the last key.
    Next(Option<Nibbles>),
}

/// Cache value storing the result of a trie cursor operation.
pub type CacheValue = Option<(Nibbles, BranchNodeCompact)>;

/// Shared caches for trie cursor operations.
#[derive(Debug, Clone)]
pub struct TrieCursorSharedCaches {
    /// Cache for account trie operations.
    pub(crate) account_cache: Arc<Cache<CacheKey, CacheValue>>,
    /// Per-address caches for storage trie operations.
    pub(crate) storage_caches: Arc<RwLock<B256Map<Arc<Cache<CacheKey, CacheValue>>>>>,
}

impl TrieCursorSharedCaches {
    /// Create new shared caches with default sizes.
    pub fn new() -> Self {
        Self::with_account_cache_size(DEFAULT_ACCOUNT_CACHE_SIZE)
    }

    /// Create new shared caches with specified account cache size.
    pub fn with_account_cache_size(account_cache_size: u64) -> Self {
        Self {
            account_cache: Arc::new(Cache::new(account_cache_size)),
            storage_caches: Arc::new(RwLock::new(B256Map::new())),
        }
    }

    /// Get or create a storage cache for the given hashed address.
    pub fn get_or_create_storage_cache(
        &self,
        hashed_address: B256,
    ) -> Arc<Cache<CacheKey, CacheValue>> {
        // Try to get with read lock first
        {
            let caches = self.storage_caches.read();
            if let Some(cache) = caches.get(&hashed_address) {
                debug!(
                    target: "trie::cached_cursor",
                    "Reusing existing storage cache for address {:?}",
                    hashed_address
                );
                return Arc::clone(cache);
            }
        }

        // Need to create - acquire write lock
        let mut caches = self.storage_caches.write();
        let cache = caches
            .entry(hashed_address)
            .or_insert_with(|| {
                debug!(
                    target: "trie::cached_cursor",
                    "Creating new storage cache for address {:?} with size {}",
                    hashed_address,
                    DEFAULT_STORAGE_CACHE_SIZE
                );
                Arc::new(Cache::new(DEFAULT_STORAGE_CACHE_SIZE))
            })
            .clone();
        cache
    }

    /// Clear all caches.
    pub fn clear(&self) {
        self.account_cache.invalidate_all();
        self.storage_caches.write().clear();
    }
}

impl Default for TrieCursorSharedCaches {
    fn default() -> Self {
        Self::new()
    }
}

/// A trie cursor factory that caches the results of cursor operations.
#[derive(Debug, Clone)]
pub struct CachedTrieCursorFactory<CF> {
    /// The underlying cursor factory.
    inner: CF,
    /// Cache for account trie operations, shared across all account cursors.
    account_cache: Arc<Cache<CacheKey, CacheValue>>,
    /// Optional reference to shared caches struct.
    shared_caches: Option<TrieCursorSharedCaches>,
}

impl<CF> CachedTrieCursorFactory<CF> {
    /// Create a new cached trie cursor factory with default cache sizes.
    pub fn new(inner: CF, shared_caches: TrieCursorSharedCaches) -> Self {
        Self {
            inner,
            account_cache: Arc::clone(&shared_caches.account_cache),
            shared_caches: Some(shared_caches.clone()),
        }
    }
}

impl<CF: TrieCursorFactory> TrieCursorFactory for CachedTrieCursorFactory<CF> {
    type AccountTrieCursor = CachedAccountTrieCursor<CF::AccountTrieCursor>;
    type StorageTrieCursor = CachedStorageTrieCursor<CF::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        let cursor = self.inner.account_trie_cursor()?;
        Ok(CachedAccountTrieCursor::new(cursor, Arc::clone(&self.account_cache)))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        let cursor = self.inner.storage_trie_cursor(hashed_address)?;
        if let Some(ref shared_caches) = self.shared_caches {
            let cache = shared_caches.get_or_create_storage_cache(hashed_address);
            return Ok(CachedStorageTrieCursor::with_external_cache(hashed_address, cursor, cache));
        }
        Ok(CachedStorageTrieCursor::new(hashed_address, cursor))
    }
}

/// A cached account trie cursor that wraps an underlying cursor and caches its results.
#[derive(Debug)]
pub struct CachedAccountTrieCursor<C> {
    /// The underlying cursor.
    inner: C,
    /// Shared cache for account trie operations.
    cache: Arc<Cache<CacheKey, CacheValue>>,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
    /// Metrics.
    #[cfg(feature = "metrics")]
    metrics: metrics::CachedTrieCursorMetrics,
}

impl<C> CachedAccountTrieCursor<C> {
    /// Create a new cached account trie cursor.
    fn new(inner: C, cache: Arc<Cache<CacheKey, CacheValue>>) -> Self {
        Self {
            inner,
            cache,
            last_key: None,
            #[cfg(feature = "metrics")]
            metrics: metrics::CachedTrieCursorMetrics::state(),
        }
    }
}

impl<C: TrieCursor> TrieCursor for CachedAccountTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::SeekExact(key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Account cursor cache hit: seek_exact(key={:?}) -> found={}",
                key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.seek_exact(key.clone())?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Account cursor cache miss: seek_exact(key={:?}) -> found={} (fetched from db)",
            key,
            result.is_some()
        );
        Ok(result)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Seek(key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Account cursor cache hit: seek(key={:?}) -> found={}",
                key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.seek(key.clone())?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Account cursor cache miss: seek(key={:?}) -> found={} (fetched from db)",
            key,
            result.is_some()
        );
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Next(self.last_key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Account cursor cache hit: next(last_key={:?}) -> found={}",
                self.last_key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.next()?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Account cursor cache miss: next(last_key={:?}) -> found={} (fetched from db)",
            self.last_key,
            result.is_some()
        );
        Ok(result)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        // We don't cache current() as it depends on cursor state
        self.inner.current()
    }
}

/// A cached storage trie cursor that wraps an underlying cursor and caches its results.
#[derive(Debug)]
pub struct CachedStorageTrieCursor<C> {
    /// The hashed address of the account.
    hashed_address: B256,
    /// The underlying cursor.
    inner: C,
    /// Cache for this specific storage trie's operations.
    cache: Arc<Cache<CacheKey, CacheValue>>,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
    /// Metrics.
    #[cfg(feature = "metrics")]
    metrics: metrics::CachedTrieCursorMetrics,
}
impl<C> CachedStorageTrieCursor<C> {
    /// Create a new cached storage trie cursor with default cache size.
    fn new(hashed_address: B256, inner: C) -> Self {
        Self::with_capacity(hashed_address, inner, DEFAULT_STORAGE_CACHE_SIZE)
    }

    /// Create a new cached storage trie cursor with specified cache capacity.
    fn with_capacity(hashed_address: B256, inner: C, cache_size: u64) -> Self {
        Self {
            hashed_address,
            inner,
            cache: Arc::new(Cache::new(cache_size)),
            last_key: None,
            #[cfg(feature = "metrics")]
            metrics: metrics::CachedTrieCursorMetrics::storage(),
        }
    }

    /// Create a new cached storage trie cursor with an external cache.
    fn with_external_cache(
        hashed_address: B256,
        inner: C,
        cache: Arc<Cache<CacheKey, CacheValue>>,
    ) -> Self {
        Self {
            hashed_address,
            inner,
            cache,
            last_key: None,
            #[cfg(feature = "metrics")]
            metrics: metrics::CachedTrieCursorMetrics::storage(),
        }
    }
}

impl<C: TrieCursor> TrieCursor for CachedStorageTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::SeekExact(key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Storage cursor cache hit for addr {:?}: seek_exact(key={:?}) -> found={}",
                self.hashed_address,
                key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.seek_exact(key.clone())?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Storage cursor cache miss for addr {:?}: seek_exact(key={:?}) -> found={} (fetched from db)",
            self.hashed_address,
            key,
            result.is_some()
        );
        Ok(result)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Seek(key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Storage cursor cache hit for addr {:?}: seek(key={:?}) -> found={}",
                self.hashed_address,
                key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.seek(key.clone())?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Storage cursor cache miss for addr {:?}: seek(key={:?}) -> found={} (fetched from db)",
            self.hashed_address,
            key,
            result.is_some()
        );
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Next(self.last_key.clone());

        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            self.metrics.hits.increment(1);
            debug!(
                target: "trie::cached_cursor",
                "Storage cursor cache hit for addr {:?}: next(last_key={:?}) -> found={}",
                self.hashed_address,
                self.last_key,
                cached.is_some()
            );
            return Ok(cached);
        }

        let result = self.inner.next()?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        self.metrics.misses.increment(1);
        debug!(
            target: "trie::cached_cursor",
            "Storage cursor cache miss for addr {:?}: next(last_key={:?}) -> found={} (fetched from db)",
            self.hashed_address,
            self.last_key,
            result.is_some()
        );
        Ok(result)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        // We don't cache current() as it depends on cursor state
        self.inner.current()
    }
}

#[cfg(feature = "metrics")]
mod metrics {
    use metrics::Gauge;
    use reth_metrics::Metrics;

    use crate::TrieType;

    #[derive(Metrics)]
    #[metrics(scope = "trie.cached_cursor")]
    pub(super) struct CachedTrieCursorMetrics {
        /// Number of cache hits
        pub(crate) hits: Gauge,
        /// Number of cache misses
        pub(crate) misses: Gauge,
    }
    impl CachedTrieCursorMetrics {
        /// Create new metrics for the state trie.
        pub(super) fn state() -> Self {
            Self::new_with_labels(&[("type", TrieType::State.as_str())])
        }

        /// Create new metrics for the storage trie.
        pub(super) fn storage() -> Self {
            Self::new_with_labels(&[("type", TrieType::Storage.as_str())])
        }
    }
}
