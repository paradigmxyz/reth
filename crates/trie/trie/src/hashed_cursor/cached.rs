use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Debug,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use alloy_primitives::{
    map::{B256Map, B256Set, FbBuildHasher},
    B256, U256,
};
use dashmap::DashMap;
use parking_lot::RwLock;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostState;
use tracing::debug;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

type Map<V> = DashMap<B256, V, FbBuildHasher<32>>;

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
    pub fn apply_hashed_post_state(&self, hashed_post_state: HashedPostState) {
        let mut cached_seeks_exact = self.account_cache.cached_seeks_exact.clone();
        let cached_seeks_inexact = self.account_cache.cached_seeks_inexact.clone();
        let cached_nexts = self.account_cache.cached_nexts.clone();

        let seeks_exact_limit = self.account_cache.seeks_exact_limit;
        let seeks_inexact_limit = self.account_cache.seeks_inexact_limit;

        std::thread::spawn(move || {
            // TODO: acquire a lock outside of the thread
            let mut cached_seeks_inexact_write = cached_seeks_inexact.write();

            let mut size = cached_seeks_exact.len();
            cached_seeks_exact.retain(|address, _| {
                if size > seeks_exact_limit && !hashed_post_state.accounts.contains_key(address) {
                    size -= 1;
                    return false;
                }

                true
            });
            cached_seeks_exact.extend(
                hashed_post_state.accounts.iter().map(|(address, account)| {
                    (*address, account.map(|account| (*address, account)))
                }),
            );
            // TODO: do not invalidate all next calls
            cached_nexts.clear();

            let start = Instant::now();
            let mut noops = 0;
            let mut cache_updates = B256Map::default();
            let mut cache_deletions = B256Set::default();
            let mut cache_invalidations = B256Set::default();

            let hashed_post_state_accounts =
                hashed_post_state.accounts.iter().collect::<BTreeMap<_, _>>();
            for entry in cached_seeks_inexact_write.iter() {
                let (&seek_address, value) = entry.pair();

                // Find first hashed post state account that is greater than or equal to the address
                // that was sought.
                let hashed_post_state_account = hashed_post_state_accounts.iter().find_map(
                    |(&&post_state_address, account)| {
                        account.and_then(|account| {
                            (post_state_address >= seek_address)
                                .then_some((post_state_address, account))
                        })
                    },
                );

                match (value, hashed_post_state_account) {
                    (Some((old_address, _)), Some((new_address, _))) => {
                        if &new_address <= old_address {
                            // First matching address from the hashed post state is less or equal to
                            // previously sought address. This means that the hashed post state
                            // account should be returned for this seek.
                            cache_updates.insert(seek_address, hashed_post_state_account);
                        } else {
                            // Nothing to do here. The most relevant address is already cached.
                            noops += 1;
                        }
                    }
                    (None, Some(_)) => {
                        // Previously sought address wasn't found, but now there's a hashed post
                        // state address that matches the key.
                        cache_updates.insert(seek_address, hashed_post_state_account);
                    }
                    (Some((old_address, _)), None) => {
                        if hashed_post_state.accounts.get(old_address) == Some(&None) {
                            // Previously sought address is now deleted. We don't know if there's an
                            // address in the underlying cursor that will match the key, so we
                            // delete the cached entry.
                            cache_deletions.insert(seek_address);
                        } else {
                            // TODO: Unsure what to do here yet.
                            cache_invalidations.insert(seek_address);
                        }
                    }
                    (None, None) => {
                        // Nothing to do here. Previously sought address wasn't found, and there's
                        // no account in the hashed post state that matches
                        // the key.
                        noops += 1;
                    }
                }
            }

            let updated = cache_updates.len();
            let deleted = cache_deletions.len();
            let invalidated = cache_invalidations.len();

            let mut size = cached_seeks_inexact_write.len();
            cached_seeks_inexact_write.retain(|address, _| {
                if size > seeks_inexact_limit && !cache_updates.contains_key(address) {
                    size -= 1;
                    return false;
                }

                true
            });

            cached_seeks_inexact_write.extend(cache_updates);
            cached_seeks_inexact_write.retain(|address, _| {
                !cache_deletions.contains(address) && !cache_invalidations.contains(address)
            });

            let seeks_exact_size = cached_seeks_exact.len();
            let seeks_inexact_size = cached_seeks_inexact_write.len();
            let nexts_size = cached_nexts.len();

            debug!(
                target: "trie::hashed_cursor::cached",
                elapsed = ?start.elapsed(),
                noops,
                updated,
                deleted,
                invalidated,
                seeks_exact_size,
                seeks_inexact_size,
                nexts_size,
                "Updated cached account inexact seeks"
            );
        });
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

        let account_seeks_exact_size = self.account_cache.cached_seeks_exact.len();
        let account_seeks_inexact_size = self.account_cache.cached_seeks_inexact.read().len();
        let account_nexts_size = self.account_cache.cached_nexts.len();

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
    /// The key is the sought key, and the value is the result of the seek.
    ///
    /// This map is also populated:
    /// - During the [`Self::seek`] calls using the key that the cursor actually sought to.
    /// - During the [`Self::next`] calls using the key that the cursor actually advanced to.
    cached_seeks_exact: Map<Option<(B256, T)>>,
    /// The cache of [`Self::seek`] calls that resulted in inexact matches.
    ///
    /// The key is the sought key, and the value is the result of the seek.
    cached_seeks_inexact: Arc<RwLock<Map<Option<(B256, T)>>>>,
    /// The cache of [`Self::next`] calls.
    ///
    /// The key is the previous key before calling [`Self::next`], and the value is the result of
    /// the call.
    cached_nexts: Map<Option<(B256, T)>>,

    seeks_exact_limit: usize,
    seeks_inexact_limit: usize,
    nexts_limit: usize,

    seeks_hit: AtomicUsize,
    seeks_total: AtomicUsize,
    nexts_hit: AtomicUsize,
    nexts_total: AtomicUsize,
}

impl<T> Default for CachedHashedCursorCache<T> {
    fn default() -> Self {
        Self {
            cached_seeks_exact: Default::default(),
            cached_seeks_inexact: Default::default(),
            cached_nexts: Default::default(),

            seeks_exact_limit: 100_000,
            seeks_inexact_limit: 100_000,
            nexts_limit: 100_000,

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
    /// Otherwise, the underlying cursor will be sought to the given key.
    ///
    /// The result of the seek will be cached, and the key that the underlying cursor sought to
    /// will be cached as well if it differs from the sought key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, C::Value)>, DatabaseError> {
        self.cache.seeks_total.fetch_add(1, Ordering::Relaxed);
        let result = if let Some(result) = self
            .cache
            .cached_seeks_exact
            .get(&key)
            .map(|v| *v)
            .or_else(|| self.cache.cached_seeks_inexact.read().get(&key).map(|v| *v))
        {
            self.cache.seeks_hit.fetch_add(1, Ordering::Relaxed);
            self.seek_before_next = true;
            result
        } else {
            self.seek_before_next = false;

            let result = self.cursor.seek(key)?;
            if let Some(actual_seek) = result.filter(|(k, _)| k != &key).map(|(k, _)| k) {
                self.cache.cached_seeks_inexact.read().insert(key, result);
                self.cache.cached_seeks_exact.insert(actual_seek, result);
            } else {
                self.cache.cached_seeks_exact.insert(key, result);
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
            *result
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
