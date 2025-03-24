use std::{fmt::Debug, sync::Arc};

use alloy_primitives::{map::FbBuildHasher, B256, U256};
use dashmap::DashMap;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

type Map<V> = DashMap<B256, V, FbBuildHasher<32>>;

/// The hashed cursor factory that creates cursors that cache the visited keys.
///
/// CAUTION: If the underlying cursor factory changes, the cache will NOT be invalidated, and the
/// old values will be returned.
#[derive(Debug, Clone)]
pub struct CachedHashedCursorFactory<CF> {
    cursor_factory: CF,
    cache: Option<Arc<CachedHashedCursorFactoryCache>>,
}

impl<CF> CachedHashedCursorFactory<CF> {
    /// Creates a new factory.
    pub fn new(cursor_factory: CF, cache: Option<Arc<CachedHashedCursorFactoryCache>>) -> Self {
        Self { cursor_factory, cache }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CachedHashedCursorFactoryCache {
    account_cache: Arc<CachedHashedCursorCache<Account>>,
    storage_cache: Map<Arc<CachedHashedCursorCache<U256>>>,
}

#[derive(Clone, Debug, Default)]
pub struct CachedHashedCursorCache<T> {
    /// The cache of [`Self::seek`] calls.
    ///
    /// The key is the seeked key, and the value is the result of the seek.
    ///
    /// This map is also populated:
    /// - During the [`Self::seek`] calls using the key that the cursor actually seeked to.
    /// - During the [`Self::next`] calls using the key that the cursor actually advanced to.
    cached_seeks: Map<Option<(B256, T)>>,
    /// The cache of [`Self::next`] calls.
    ///
    /// The key is the previous key before calling [`Self::next`], and the value is the result of
    /// the call.
    cached_nexts: Map<Option<(B256, T)>>,
}

#[derive(Debug)]
pub enum CachedHashedCursorCacheChange {
    Account(CachedHashedCursorCache<Account>),
    Storage(B256, CachedHashedCursorCache<U256>),
}

impl<CF: HashedCursorFactory> HashedCursorFactory for CachedHashedCursorFactory<CF> {
    type AccountCursor = CachedHashedCursor<CF::AccountCursor, Account>;
    type StorageCursor = CachedHashedCursor<CF::StorageCursor, U256>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(CachedHashedCursor::new(
            self.cursor_factory.hashed_account_cursor()?,
            self.cache.as_ref().map(|cache| cache.account_cache.clone()),
        ))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(CachedHashedCursor::new(
            self.cursor_factory.hashed_storage_cursor(hashed_address)?,
            self.cache
                .as_ref()
                .and_then(|cache| cache.storage_cache.get(&hashed_address))
                .map(|v| v.clone()),
        ))
    }
}

/// The hashed cursor that caches the visited keys.
#[derive(Debug)]
pub struct CachedHashedCursor<C, T: Clone> {
    cursor: C,
    /// Last visited key.
    last_key: Option<B256>,
    seek_cursor: bool,
    cache: Option<Arc<CachedHashedCursorCache<T>>>,
}

impl<C, T> CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedCursor<Value = T>,
{
    fn new(cursor: C, cache: Option<Arc<CachedHashedCursorCache<T>>>) -> Self {
        Self { cursor, last_key: None, seek_cursor: false, cache }
    }

    fn get_cached_seek(&self, key: B256) -> Option<Option<(B256, C::Value)>> {
        self.cache.as_ref().and_then(|cache| cache.cached_seeks.get(&key)).map(|v| *v)
    }

    fn get_cached_next(&self, key: B256) -> Option<Option<(B256, C::Value)>> {
        self.cache.as_ref().and_then(|cache| cache.cached_nexts.get(&key)).map(|v| *v)
    }
}

impl<C, T> HashedCursor for CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedCursor<Value = T>,
{
    type Value = T;

    /// Seeks to the given key.
    ///
    /// If the key is already cached, the value will be returned from the cache.
    /// Otherwise, the underlying cursor will be seeked to the given key.
    ///
    /// The result of the seek will be cached, and the key that the underlying cursor seeked to
    /// will be cached as well if it differs from the seeked key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, C::Value)>, DatabaseError> {
        let result = if let Some(result) = self.get_cached_seek(key) {
            self.seek_cursor = true;
            result
        } else {
            self.seek_cursor = false;
            let result = self.cursor.seek(key)?;
            if let Some(cache) = self.cache.as_ref() {
                cache.cached_seeks.insert(key, result);

                let actual_seek = result.filter(|(k, _)| k != &key).map(|(k, _)| k);
                if let Some(actual_seek) = actual_seek {
                    cache.cached_seeks.insert(actual_seek, result);
                }
            }
            result
        };

        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    /// Advances to the next key.
    ///
    /// If the next value after the current key is already cached, it will be returned from the
    /// cache. Otherwise, the underlying cursor will be advanced to the next key.
    ///
    /// The result of the advance will be cached in both the cache of [`Self::seek`] calls and the
    /// cache of [`Self::next`] calls.
    fn next(&mut self) -> Result<Option<(B256, C::Value)>, DatabaseError> {
        let Some(last_key) = self.last_key else { return Ok(None) };

        let result = if let Some(result) = self.get_cached_next(last_key) {
            self.seek_cursor = true;
            result
        } else {
            if self.seek_cursor {
                self.seek_cursor = false;
                self.cursor.seek(last_key)?;
            }
            let result = self.cursor.next()?;
            if let Some(cache) = self.cache.as_ref() {
                cache.cached_nexts.insert(last_key, result);
                if let Some((key, value)) = result.as_ref() {
                    cache.cached_seeks.insert(*key, Some((*key, *value)));
                }
            }
            result
        };

        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }
}

impl<C, T> HashedStorageCursor for CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedStorageCursor<Value = T>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.cursor.is_storage_empty()
    }
}
