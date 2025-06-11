use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use mini_moka::sync::Cache;
use parking_lot::RwLock;
use reth_storage_errors::db::DatabaseError;
use tracing::debug;
use std::{collections::HashMap, sync::Arc};

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
    pub(crate) storage_caches: Arc<RwLock<HashMap<B256, Arc<Cache<CacheKey, CacheValue>>>>>,
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
            storage_caches: Arc::new(RwLock::new(HashMap::new())),
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
    pub fn new(inner: CF) -> Self {
        Self::with_capacity(inner, DEFAULT_ACCOUNT_CACHE_SIZE)
    }

    /// Create a new cached trie cursor factory with specified cache capacity for account
    /// operations.
    pub fn with_capacity(inner: CF, account_cache_size: u64) -> Self {
        Self { inner, account_cache: Arc::new(Cache::new(account_cache_size)), shared_caches: None }
    }

    /// Create a new cached trie cursor factory with shared caches.
    pub fn with_shared_caches(inner: CF, shared_caches: &TrieCursorSharedCaches) -> Self {
        debug!(
            target: "trie::cached_cursor",
            "Creating CachedTrieCursorFactory with shared caches"
        );
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
}

impl<C> CachedAccountTrieCursor<C> {
    /// Create a new cached account trie cursor.
    const fn new(inner: C, cache: Arc<Cache<CacheKey, CacheValue>>) -> Self {
        Self { inner, cache, last_key: None }
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
    #[allow(dead_code)]
    hashed_address: B256,
    /// The underlying cursor.
    inner: C,
    /// Cache for this specific storage trie's operations.
    cache: StorageCache,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
}

/// Storage cache can be either owned or shared.
#[derive(Debug)]
enum StorageCache {
    /// Owned cache instance.
    Owned(Cache<CacheKey, CacheValue>),
    /// Shared cache instance.
    Shared(Arc<Cache<CacheKey, CacheValue>>),
}

impl StorageCache {
    fn get(&self, key: &CacheKey) -> Option<CacheValue> {
        match self {
            Self::Owned(cache) => cache.get(key),
            Self::Shared(cache) => cache.get(key),
        }
    }

    fn insert(&self, key: CacheKey, value: CacheValue) {
        match self {
            Self::Owned(cache) => cache.insert(key, value),
            Self::Shared(cache) => cache.insert(key, value),
        }
    }
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
            cache: StorageCache::Owned(Cache::new(cache_size)),
            last_key: None,
        }
    }

    /// Create a new cached storage trie cursor with an external cache.
    const fn with_external_cache(
        hashed_address: B256,
        inner: C,
        cache: Arc<Cache<CacheKey, CacheValue>>,
    ) -> Self {
        Self { hashed_address, inner, cache: StorageCache::Shared(cache), last_key: None }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::noop::NoopTrieCursorFactory;

    #[test]
    fn test_cached_account_cursor() {
        let factory = CachedTrieCursorFactory::new(NoopTrieCursorFactory);
        let mut cursor = factory.account_trie_cursor().unwrap();

        // First call should miss cache and return None (noop cursor)
        let key = Nibbles::unpack(B256::from([1u8; 32]));
        assert!(cursor.seek_exact(key.clone()).unwrap().is_none());

        // Second call with same key should hit cache
        assert!(cursor.seek_exact(key).unwrap().is_none());
    }

    #[test]
    fn test_cached_storage_cursor() {
        let factory = CachedTrieCursorFactory::new(NoopTrieCursorFactory);
        let hashed_address = B256::from([1u8; 32]);
        let mut cursor = factory.storage_trie_cursor(hashed_address).unwrap();

        // First call should miss cache and return None (noop cursor)
        let key = Nibbles::unpack(B256::from([2u8; 32]));
        assert!(cursor.seek(key.clone()).unwrap().is_none());

        // Second call with same key should hit cache
        assert!(cursor.seek(key).unwrap().is_none());
    }

    #[test]
    fn test_separate_storage_caches() {
        let factory = CachedTrieCursorFactory::new(NoopTrieCursorFactory);

        // Create two storage cursors for different addresses
        let addr1 = B256::from([1u8; 32]);
        let addr2 = B256::from([2u8; 32]);
        let mut cursor1 = factory.storage_trie_cursor(addr1).unwrap();
        let mut cursor2 = factory.storage_trie_cursor(addr2).unwrap();

        let key = Nibbles::unpack(B256::from([3u8; 32]));

        // Operations on different cursors should not share cache
        assert!(cursor1.seek(key.clone()).unwrap().is_none());
        assert!(cursor2.seek(key).unwrap().is_none());
    }
}
