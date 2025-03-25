use std::{
    fmt::Debug,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use alloy_primitives::{map::FbBuildHasher, B256, U256};
use mini_moka::sync::CacheBuilder;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostState;
use tracing::debug;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

pub type Cache<V> = mini_moka::sync::Cache<B256, V, FbBuildHasher<32>>;

/// The hashed cursor factory that creates cursors that cache the visited keys.
///
/// CAUTION: If the underlying cursor factory changes, the cache will NOT be invalidated, and the
/// old values will be returned.
#[derive(Debug, Clone)]
pub struct CachedHashedCursorFactory<CF> {
    cursor_factory: CF,
    cache: CachedHashedCursorFactoryCache,
}

impl<CF> CachedHashedCursorFactory<CF> {
    /// Creates a new factory.
    pub fn new(cursor_factory: CF, cache: CachedHashedCursorFactoryCache) -> Self {
        Self { cursor_factory, cache }
    }
}

#[derive(Clone, Debug)]
pub struct CachedHashedCursorFactoryCache {
    account_cache: Arc<CachedHashedCursorCache<Account>>,
    // storage_cache: Arc<Map<Arc<CachedHashedCursorCache<U256>>>>,
}

impl Default for CachedHashedCursorFactoryCache {
    fn default() -> Self {
        Self { account_cache: Arc::new(CachedHashedCursorCache::default()) }
    }
}

impl CachedHashedCursorFactoryCache {
    pub fn apply_hashed_post_state(&self, hashed_post_state: &HashedPostState) {
        self.account_cache.cached_nexts.invalidate_all();
        self.account_cache.cached_seeks_inexact.invalidate_all();
        for (address, account) in &hashed_post_state.accounts {
            self.account_cache
                .cached_seeks_exact
                .insert(*address, account.map(|account| (*address, account)));
        }
    }

    pub fn reset_metrics(&self) {
        self.account_cache.reset_metrics();
    }

    pub fn log_stats(&self) {
        let account_seeks_hit = self.account_cache.seeks_hit.load(Ordering::Relaxed);
        let account_seeks_total = self.account_cache.seeks_total.load(Ordering::Relaxed);
        let account_nexts_hit = self.account_cache.nexts_hit.load(Ordering::Relaxed);
        let account_nexts_total = self.account_cache.nexts_total.load(Ordering::Relaxed);
        // let (storage_seeks_hit, storage_seeks_total, storage_nexts_hit, storage_nexts_total) =
        //     self.storage_cache.iter().fold(
        //         (0, 0, 0, 0),
        //         |(acc_seeks_hit, acc_seeks_total, acc_nexts_hit, acc_nexts_total), entry| {
        //             let cache = entry.value();
        //             (
        //                 acc_seeks_hit + cache.seeks_hit.load(Ordering::Relaxed),
        //                 acc_seeks_total + cache.seeks_total.load(Ordering::Relaxed),
        //                 acc_nexts_hit + cache.nexts_hit.load(Ordering::Relaxed),
        //                 acc_nexts_total + cache.nexts_total.load(Ordering::Relaxed),
        //             )
        //         },
        //     );

        let account_seeks_exact_size = self.account_cache.cached_seeks_exact.entry_count();
        let account_seeks_inexact_size = self.account_cache.cached_seeks_inexact.entry_count();
        let account_nexts_size = self.account_cache.cached_nexts.entry_count();

        debug!(
            target: "trie::hashed_cursor::cached",
            account_seeks_hit,
            account_seeks_total,
            account_nexts_hit,
            account_nexts_total,
            // storage_seeks_hit,
            // storage_seeks_total,
            // storage_nexts_hit,
            // storage_nexts_total,
            account_seeks_exact_size,
            account_seeks_inexact_size,
            account_nexts_size,
            "CachedHashedCursorFactoryCache raw stats"
        );

        let account_seeks_hitrate = account_seeks_hit as f64 / account_seeks_total as f64;
        let account_nexts_hitrate = account_nexts_hit as f64 / account_nexts_total as f64;
        // let storage_seeks_hitrate = storage_seeks_hit as f64 / storage_seeks_total as f64;
        // let storage_nexts_hitrate = storage_nexts_hit as f64 / storage_nexts_total as f64;

        debug!(
            target: "trie::hashed_cursor::cached",
            account_seeks_hitrate,
            account_nexts_hitrate,
            // storage_seeks_hitrate,
            // storage_nexts_hitrate,
            "CachedHashedCursorFactoryCache hitrates"
        );
    }
}

#[derive(Debug)]
pub struct CachedHashedCursorCache<T> {
    /// The cache of [`Self::seek`] calls that resulted in exact matches.
    ///
    /// The key is the seeked key, and the value is the result of the seek.
    ///
    /// This map is also populated:
    /// - During the [`Self::seek`] calls using the key that the cursor actually seeked to.
    /// - During the [`Self::next`] calls using the key that the cursor actually advanced to.
    cached_seeks_exact: Cache<Option<(B256, T)>>,
    /// The cache of [`Self::seek`] calls that resulted in inexact matches.
    ///
    /// The key is the seeked key, and the value is the result of the seek.
    cached_seeks_inexact: Cache<Option<(B256, T)>>,
    seeks_hit: AtomicUsize,
    seeks_total: AtomicUsize,
    /// The cache of [`Self::next`] calls.
    ///
    /// The key is the previous key before calling [`Self::next`], and the value is the result of
    /// the call.
    cached_nexts: Cache<Option<(B256, T)>>,
    nexts_hit: AtomicUsize,
    nexts_total: AtomicUsize,
}

impl<T: Clone + Send + Sync + 'static> Default for CachedHashedCursorCache<T> {
    fn default() -> Self {
        Self {
            cached_seeks_exact: CacheBuilder::new(10_000)
                .build_with_hasher(FbBuildHasher::default()),
            cached_seeks_inexact: CacheBuilder::new(10_000)
                .build_with_hasher(FbBuildHasher::default()),
            cached_nexts: CacheBuilder::new(10_000).build_with_hasher(FbBuildHasher::default()),
            seeks_hit: AtomicUsize::new(0),
            seeks_total: AtomicUsize::new(0),
            nexts_hit: AtomicUsize::new(0),
            nexts_total: AtomicUsize::new(0),
        }
    }
}

impl<T> CachedHashedCursorCache<T> {
    pub fn reset_metrics(&self) {
        self.seeks_hit.store(0, Ordering::Relaxed);
        self.seeks_total.store(0, Ordering::Relaxed);
        self.nexts_hit.store(0, Ordering::Relaxed);
        self.nexts_total.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub enum CachedHashedCursorCacheChange {
    Account(CachedHashedCursorCache<Account>),
    Storage(B256, CachedHashedCursorCache<U256>),
}

impl<CF: HashedCursorFactory> HashedCursorFactory for CachedHashedCursorFactory<CF> {
    type AccountCursor = CachedHashedCursor<CF::AccountCursor, Account>;
    type StorageCursor = CF::StorageCursor;
    // type StorageCursor = CachedHashedCursor<CF::StorageCursor, U256>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(CachedHashedCursor::new(
            self.cursor_factory.hashed_account_cursor()?,
            self.cache.account_cache.clone(),
        ))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        self.cursor_factory.hashed_storage_cursor(hashed_address)
        // Ok(CachedHashedCursor::new(
        //     self.cursor_factory.hashed_storage_cursor(hashed_address)?,
        //     self.cache.storage_cache.entry(hashed_address).or_default().clone(),
        // ))
    }
}

/// The hashed cursor that caches the visited keys.
#[derive(Debug)]
pub struct CachedHashedCursor<C, T: Clone> {
    cursor: C,
    cache: Arc<CachedHashedCursorCache<T>>,
    /// Last visited key.
    last_key: Option<B256>,
    seek_before_next: bool,
}

impl<C, T> CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default,
    C: HashedCursor<Value = T>,
{
    fn new(cursor: C, cache: Arc<CachedHashedCursorCache<T>>) -> Self {
        Self { cursor, cache, last_key: None, seek_before_next: false }
    }
}

impl<C, T> HashedCursor for CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default + Send + Sync + 'static,
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
        self.cache.seeks_total.fetch_add(1, Ordering::Relaxed);
        let result = if let Some(result) = self.cache.cached_seeks_exact.get(&key) {
            self.cache.seeks_hit.fetch_add(1, Ordering::Relaxed);
            self.seek_before_next = true;
            result
        } else {
            self.seek_before_next = false;

            let result = self.cursor.seek(key)?;
            if result.filter(|(k, _)| k == &key).is_some() {
                self.cache.cached_seeks_exact.insert(key, result);
            } else {
                self.cache.cached_seeks_inexact.insert(key, result);
            }

            let actual_seek = result.filter(|(k, _)| k != &key).map(|(k, _)| k);
            if let Some(actual_seek) = actual_seek {
                self.cache.cached_seeks_exact.insert(actual_seek, result);
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

        self.cache.nexts_total.fetch_add(1, Ordering::Relaxed);
        let result = if let Some(result) = self.cache.cached_nexts.get(&last_key) {
            self.cache.nexts_hit.fetch_add(1, Ordering::Relaxed);
            self.seek_before_next = true;
            result
        } else {
            if self.seek_before_next {
                self.seek_before_next = false;
                self.cursor.seek(last_key)?;
            }

            let result = self.cursor.next()?;
            self.cache.cached_nexts.insert(last_key, result);
            if let Some((key, value)) = result.as_ref() {
                self.cache.cached_seeks_exact.insert(*key, Some((*key, *value)));
            }
            result
        };

        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }
}

impl<C, T> HashedStorageCursor for CachedHashedCursor<C, T>
where
    T: Debug + Clone + Copy + Default + Send + Sync + 'static,
    C: HashedStorageCursor<Value = T>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.cursor.is_storage_empty()
    }
}
