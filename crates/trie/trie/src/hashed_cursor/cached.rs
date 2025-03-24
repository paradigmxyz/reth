use std::{fmt::Debug, sync::mpsc};

use alloy_primitives::{map::B256Map, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

/// The hashed cursor factory that creates cursors that cache the visited keys.
///
/// CAUTION: If the underlying cursor factory changes, the cache will NOT be invalidated, and the
/// old values will be returned.
#[derive(Debug, Clone)]
pub struct CachedHashedCursorFactory<'a, CF> {
    cursor_factory: CF,
    cache: Option<&'a CachedHashedCursorFactoryCache>,
    cache_changes_tx: mpsc::Sender<CachedHashedCursorCacheChange>,
}

impl<'a, CF> CachedHashedCursorFactory<'a, CF> {
    /// Creates a new factory.
    pub fn new(
        cursor_factory: CF,
        cache: Option<&'a CachedHashedCursorFactoryCache>,
    ) -> (Self, mpsc::Receiver<CachedHashedCursorCacheChange>) {
        let (cache_changes_tx, cache_changes_rx) = mpsc::channel();
        (Self { cursor_factory, cache, cache_changes_tx }, cache_changes_rx)
    }

    pub fn take_cache_changes(
        self,
        cache_changes_rx: mpsc::Receiver<CachedHashedCursorCacheChange>,
    ) -> CachedHashedCursorFactoryCache {
        drop(self.cache_changes_tx);
        cache_changes_rx.iter().fold(
            CachedHashedCursorFactoryCache::default(),
            |mut acc, change| {
                match change {
                    CachedHashedCursorCacheChange::Account(cache) => {
                        acc.account_cache.extend(cache)
                    }
                    CachedHashedCursorCacheChange::Storage(hashed_address, cache) => {
                        acc.storage_cache.entry(hashed_address).or_default().extend(cache);
                    }
                }
                acc
            },
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct CachedHashedCursorFactoryCache {
    account_cache: CachedHashedCursorCache<Account>,
    storage_cache: B256Map<CachedHashedCursorCache<U256>>,
}

impl CachedHashedCursorFactoryCache {
    pub fn extend(&mut self, other: Self) {
        self.account_cache.extend(other.account_cache);
        for (hashed_address, cache) in other.storage_cache {
            self.storage_cache.entry(hashed_address).or_default().extend(cache);
        }
    }
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
    cached_seeks: B256Map<Option<(B256, T)>>,
    /// The cache of [`Self::next`] calls.
    ///
    /// The key is the previous key before calling [`Self::next`], and the value is the result of
    /// the call.
    cached_nexts: B256Map<Option<(B256, T)>>,
    /// The cache of [`Self::is_storage_empty`] call.
    is_storage_empty: Option<bool>,
}

impl<T> CachedHashedCursorCache<T> {
    fn extend(&mut self, other: Self) {
        self.cached_seeks.extend(other.cached_seeks);
        self.cached_nexts.extend(other.cached_nexts);
        if let Some(other) = other.is_storage_empty {
            self.is_storage_empty = Some(other);
        }
    }
}

pub enum CachedHashedCursorCacheChange {
    Account(CachedHashedCursorCache<Account>),
    Storage(B256, CachedHashedCursorCache<U256>),
}

impl<'a, CF: HashedCursorFactory> HashedCursorFactory for CachedHashedCursorFactory<'a, CF> {
    type AccountCursor = CachedHashedAccountCursor<'a, CF::AccountCursor>;
    type StorageCursor = CachedHashedStorageCursor<'a, CF::StorageCursor>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(CachedHashedAccountCursor::new(CachedHashedCursor::new(
            self.cursor_factory.hashed_account_cursor()?,
            self.cache.as_ref().map(|cache| &cache.account_cache),
            self.cache_changes_tx.clone(),
        )))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(CachedHashedStorageCursor::new(
            hashed_address,
            CachedHashedCursor::new(
                self.cursor_factory.hashed_storage_cursor(hashed_address)?,
                self.cache.as_ref().and_then(|cache| cache.storage_cache.get(&hashed_address)),
                self.cache_changes_tx.clone(),
            ),
        ))
    }
}

#[derive(Debug)]
pub struct CachedHashedAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    inner: CachedHashedCursor<'a, C, Account>,
}

impl<'a, C> CachedHashedAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    fn new(inner: CachedHashedCursor<'a, C, Account>) -> Self {
        Self { inner }
    }
}

impl<'a, C> HashedCursor for CachedHashedAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    type Value = Account;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.inner.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.inner.next()
    }
}

impl<'a, C> Drop for CachedHashedAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    fn drop(&mut self) {
        self.inner
            .cache_changes_tx
            .send(CachedHashedCursorCacheChange::Account(std::mem::take(
                &mut self.inner.cache_changes,
            )))
            .unwrap();
    }
}

#[derive(Debug)]
pub struct CachedHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    hashed_address: B256,
    inner: CachedHashedCursor<'a, C, U256>,
}

impl<'a, C> CachedHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    fn new(hashed_address: B256, inner: CachedHashedCursor<'a, C, U256>) -> Self {
        Self { hashed_address, inner }
    }
}

impl<'a, C> HashedCursor for CachedHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    type Value = U256;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.inner.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.inner.next()
    }
}

impl<'a, C> HashedStorageCursor for CachedHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.inner.is_storage_empty()
    }
}

impl<'a, C> Drop for CachedHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    fn drop(&mut self) {
        self.inner
            .cache_changes_tx
            .send(CachedHashedCursorCacheChange::Storage(
                self.hashed_address,
                std::mem::take(&mut self.inner.cache_changes),
            ))
            .unwrap();
    }
}

/// The hashed cursor that caches the visited keys.
#[derive(Debug)]
struct CachedHashedCursor<'a, C, T: Clone> {
    cursor: C,
    /// The current key.
    ///
    /// Set to `B256::ZERO` if the cursor has just been created, is empty, has been seeked to a
    /// non-existent key, or has been advanced beyond the last key.
    current_key: B256,
    cache: Option<&'a CachedHashedCursorCache<T>>,
    cache_changes: CachedHashedCursorCache<T>,
    cache_changes_tx: mpsc::Sender<CachedHashedCursorCacheChange>,
}

impl<'a, C, T> CachedHashedCursor<'a, C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedCursor<Value = T>,
{
    fn new(
        cursor: C,
        cache: Option<&'a CachedHashedCursorCache<T>>,
        cache_changes_tx: mpsc::Sender<CachedHashedCursorCacheChange>,
    ) -> Self {
        Self {
            cursor,
            current_key: B256::ZERO,
            cache,
            cache_changes: Default::default(),
            cache_changes_tx,
        }
    }

    /// Seeks to the given key.
    ///
    /// If the key is already cached, the value will be returned from the cache.
    /// Otherwise, the underlying cursor will be seeked to the given key.
    ///
    /// The result of the seek will be cached, and the key that the underlying cursor seeked to
    /// will be cached as well if it differs from the seeked key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, C::Value)>, DatabaseError> {
        if let Some(result) = self.cache.as_ref().and_then(|cache| cache.cached_seeks.get(&key)) {
            return Ok(*result)
        }
        if let Some(result) = self.cache_changes.cached_seeks.get(&key) {
            return Ok(*result)
        }

        let result = self.cursor.seek(key)?;
        self.cache_changes.cached_seeks.insert(key, result);

        let actual_seek = result.filter(|(k, _)| k != &key).map(|(k, _)| k);
        if let Some(actual_seek) = actual_seek {
            self.cache_changes.cached_seeks.insert(actual_seek, result);
        }

        self.current_key = result.as_ref().map(|(k, _)| *k).unwrap_or(B256::ZERO);
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
        if let Some(result) =
            self.cache.as_ref().and_then(|cache| cache.cached_nexts.get(&self.current_key))
        {
            return Ok(*result)
        }
        if let Some(result) = self.cache_changes.cached_nexts.get(&self.current_key) {
            return Ok(*result)
        }

        let result = self.cursor.next()?;
        self.cache_changes.cached_nexts.insert(self.current_key, result);

        if let Some((key, value)) = result.as_ref() {
            self.cache_changes.cached_seeks.insert(*key, Some((*key, *value)));
        }

        self.current_key = result.as_ref().map(|(k, _)| *k).unwrap_or(B256::ZERO);
        Ok(result)
    }
}

impl<'a, C, T> CachedHashedCursor<'a, C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedStorageCursor<Value = T>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        if let Some(is_storage_empty) = self.cache.as_ref().and_then(|cache| cache.is_storage_empty)
        {
            return Ok(is_storage_empty)
        }
        if let Some(is_storage_empty) = self.cache_changes.is_storage_empty {
            return Ok(is_storage_empty)
        }

        let is_storage_empty = self.cursor.is_storage_empty()?;
        self.cache_changes.is_storage_empty = Some(is_storage_empty);
        Ok(is_storage_empty)
    }
}
