use super::{TrieCursor, TrieCursorFactory};
use crate::{updates::TrieUpdates, BranchNodeCompact, Nibbles};
use alloy_primitives::{map::B256Map, B256};
use parking_lot::RwLock;
use reth_storage_errors::db::DatabaseError;
use std::{ops::Deref, sync::Arc};
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

/// Cache for trie cursor.
#[derive(Debug, Clone)]
pub struct Cache(Arc<mini_moka::sync::Cache<CacheKey, CacheValue>>);

impl Deref for Cache {
    type Target = Arc<mini_moka::sync::Cache<CacheKey, CacheValue>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Cache {
    /// Create a new cache with the given size.
    pub fn new(size: u64) -> Self {
        Self(Arc::new(mini_moka::sync::Cache::new(size)))
    }

    /// Invalidate all entries that match the given path.
    pub fn invalidate_by_path(&self, path: &Nibbles) {
        // Remove entries for SeekExact and Seek operations with this path
        self.0.invalidate(&CacheKey::SeekExact(path.clone()));
        self.0.invalidate(&CacheKey::Seek(path.clone()));

        // For Next operations, we need to invalidate entries where the last key
        // could be affected by this path change
        // Note: This is a conservative approach - we could optimize further
        self.0.invalidate(&CacheKey::Next(Some(path.clone())));
    }

    /// Update all entries for the given path with the new node.
    pub fn update_by_path(&self, path: &Nibbles, node: BranchNodeCompact) {
        // Update SeekExact entries
        if let Some(existing) = self.0.get(&CacheKey::SeekExact(path.clone())) {
            if existing.is_some() {
                self.0
                    .insert(CacheKey::SeekExact(path.clone()), Some((path.clone(), node.clone())));
            }
        }

        // Update Seek entries - this is more complex as Seek might return this node
        // even if seeking for a different key
        if let Some(existing) = self.0.get(&CacheKey::Seek(path.clone())) {
            if existing.is_some() {
                self.0.insert(CacheKey::Seek(path.clone()), Some((path.clone(), node.clone())));
            }
        }
    }
}

/// Shared caches for trie cursor operations.
#[derive(Debug, Clone)]
pub struct TrieCursorSharedCaches {
    /// Cache for account trie operations.
    pub(crate) account_cache: Cache,
    /// Per-address caches for storage trie operations.
    pub(crate) storage_caches: Arc<RwLock<B256Map<Cache>>>,
}

impl TrieCursorSharedCaches {
    /// Create new shared caches with default sizes.
    pub fn new() -> Self {
        Self {
            account_cache: Cache::new(DEFAULT_ACCOUNT_CACHE_SIZE),
            storage_caches: Arc::new(RwLock::new(B256Map::default())),
        }
    }

    /// Get or create a storage cache for the given hashed address.
    pub fn get_or_create_storage_cache(&self, hashed_address: B256) -> Cache {
        // Try to get with read lock first
        {
            let caches = self.storage_caches.read();
            if let Some(cache) = caches.get(&hashed_address).cloned() {
                debug!(
                    target: "trie::cached_cursor",
                    "Reusing existing storage cache for address {:?}",
                    hashed_address
                );
                return cache;
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
                Cache::new(DEFAULT_STORAGE_CACHE_SIZE)
            })
            .clone();
        cache
    }

    /// Clear all caches.
    pub fn clear(&self) {
        self.account_cache.invalidate_all();
        self.storage_caches.write().clear();
    }

    /// Apply trie updates to the caches instead of clearing them.
    pub fn apply_updates(&self, updates: &TrieUpdates) {
        debug!(
            target: "trie::cached_cursor",
            "Applying trie updates to caches: {} account nodes, {} removed nodes, {} storage tries",
            updates.account_nodes.len(),
            updates.removed_nodes.len(),
            updates.storage_tries.len()
        );

        // Apply account trie updates
        // First, invalidate removed nodes
        for removed_path in &updates.removed_nodes {
            self.account_cache.invalidate_by_path(removed_path);
        }

        // Then, update modified nodes
        for (path, node) in &updates.account_nodes {
            self.account_cache.update_by_path(path, node.clone());
        }

        // Apply storage trie updates
        for (hashed_address, storage_updates) in &updates.storage_tries {
            if storage_updates.is_deleted {
                // If the storage trie is deleted, remove the entire cache for this address
                self.storage_caches.write().remove(hashed_address);
            }

            // Apply updates to existing storage cache
            let storage_caches = self.storage_caches.read();
            if let Some(cache) = storage_caches.get(hashed_address) {
                for removed_path in &storage_updates.removed_nodes {
                    cache.invalidate_by_path(removed_path);
                }

                for (path, node) in &storage_updates.storage_nodes {
                    cache.update_by_path(path, node.clone());
                }
            }
            // If there's no cache for this address yet, we don't need to do anything
        }
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
    account_cache: Cache,
    /// Optional reference to shared caches struct.
    shared_caches: TrieCursorSharedCaches,
}

impl<CF> CachedTrieCursorFactory<CF> {
    /// Create a new cached trie cursor factory with default cache sizes.
    pub fn new(inner: CF, shared_caches: TrieCursorSharedCaches) -> Self {
        Self { inner, account_cache: shared_caches.account_cache.clone(), shared_caches }
    }
}

impl<CF: TrieCursorFactory> TrieCursorFactory for CachedTrieCursorFactory<CF> {
    type AccountTrieCursor = CachedAccountTrieCursor<CF::AccountTrieCursor>;
    type StorageTrieCursor = CachedStorageTrieCursor<CF::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        let cursor = self.inner.account_trie_cursor()?;
        Ok(CachedAccountTrieCursor::new(cursor, self.account_cache.clone()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        let cursor = self.inner.storage_trie_cursor(hashed_address)?;
        Ok(CachedStorageTrieCursor::new(
            hashed_address,
            cursor,
            self.shared_caches.account_cache.clone(),
        ))
    }
}

/// A cached account trie cursor that wraps an underlying cursor and caches its results.
#[derive(Debug)]
pub struct CachedAccountTrieCursor<C> {
    /// The underlying cursor.
    inner: C,
    /// Shared cache for account trie operations.
    cache: Cache,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
    /// Metrics.
    #[cfg(feature = "metrics")]
    metrics: metrics::CachedTrieCursorMetrics,
}

impl<C> CachedAccountTrieCursor<C> {
    /// Create a new cached account trie cursor.
    fn new(inner: C, cache: Cache) -> Self {
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
    cache: Cache,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
    /// Metrics.
    #[cfg(feature = "metrics")]
    metrics: metrics::CachedTrieCursorMetrics,
}

impl<C> CachedStorageTrieCursor<C> {
    /// Create a new cached storage trie cursor with default cache size.
    fn new(hashed_address: B256, inner: C, cache: Cache) -> Self {
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
            #[cfg(feature = "metrics")]
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
        #[cfg(feature = "metrics")]
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
