use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use mini_moka::sync::Cache;
use reth_storage_errors::db::DatabaseError;
use std::sync::Arc;

/// Default cache size for account trie operations.
const DEFAULT_ACCOUNT_CACHE_SIZE: u64 = 10_000;

/// Default cache size for storage trie operations.
const DEFAULT_STORAGE_CACHE_SIZE: u64 = 1_000;

/// Cache key for trie cursor operations.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum CacheKey {
    /// Seek exact operation with the given key.
    SeekExact(Nibbles),
    /// Seek operation with the given key.
    Seek(Nibbles),
    /// Next operation with the last key.
    Next(Option<Nibbles>),
}

/// Cache value storing the result of a trie cursor operation.
type CacheValue = Option<(Nibbles, BranchNodeCompact)>;

/// A trie cursor factory that caches the results of cursor operations.
#[derive(Debug, Clone)]
pub struct CachedTrieCursorFactory<CF> {
    /// The underlying cursor factory.
    inner: CF,
    /// Cache for account trie operations, shared across all account cursors.
    account_cache: Arc<Cache<CacheKey, CacheValue>>,
}

impl<CF> CachedTrieCursorFactory<CF> {
    /// Create a new cached trie cursor factory with default cache sizes.
    pub fn new(inner: CF) -> Self {
        Self::with_capacity(inner, DEFAULT_ACCOUNT_CACHE_SIZE)
    }

    /// Create a new cached trie cursor factory with specified cache capacity for account operations.
    pub fn with_capacity(inner: CF, account_cache_size: u64) -> Self {
        Self {
            inner,
            account_cache: Arc::new(Cache::new(account_cache_size)),
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
    fn new(inner: C, cache: Arc<Cache<CacheKey, CacheValue>>) -> Self {
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
            return Ok(cached);
        }

        let result = self.inner.seek_exact(key)?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        Ok(result)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Seek(key.clone());
        
        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            return Ok(cached);
        }

        let result = self.inner.seek(key)?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Next(self.last_key.clone());
        
        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            return Ok(cached);
        }

        let result = self.inner.next()?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
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
    cache: Cache<CacheKey, CacheValue>,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
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
            cache: Cache::new(cache_size),
            last_key: None,
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
            return Ok(cached);
        }

        let result = self.inner.seek_exact(key)?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        Ok(result)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Seek(key.clone());
        
        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            return Ok(cached);
        }

        let result = self.inner.seek(key)?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let cache_key = CacheKey::Next(self.last_key.clone());
        
        if let Some(cached) = self.cache.get(&cache_key) {
            self.last_key = cached.as_ref().map(|(nibbles, _)| nibbles.clone());
            return Ok(cached);
        }

        let result = self.inner.next()?;
        self.last_key = result.as_ref().map(|(nibbles, _)| nibbles.clone());
        self.cache.insert(cache_key, result.clone());
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