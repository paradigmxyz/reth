//! Long lived state cache.
//!
//! This cache stores accounts, bytecodes, and storage slots during its lifetime. It is intended to
//! live for as long as possible in order to reduce the number of reads from the database.
//!
//! The state cache itself ([`LruStateCache`]) is just a store: in order to use it with an
//! executor, it must be constructed into a state provider by composing it on top of another state
//! provider (see [`CachedStateProvider`]).
//!
//! You must commit changes to the state to the cache after each transaction, otherwise the executor
//! will be using outdated state. Thus, if the cache is updated every block, precautions must be
//! taken to ensure that there is a caching layer on top of this cache that has the latest
//! intra-block state available.

use crate::{
    post_state::Storage, AccountProvider, BlockHashProvider, PostState, StateProvider,
    StateRootProvider,
};
use metrics::Counter;
use reth_interfaces::Result;
use reth_metrics_derive::Metrics;
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, H256, U256,
};
use schnellru::{ByLength, LruMap};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// A state cache for EVM objects backed by multiple LRU caches.
pub struct LruStateCache {
    account_cache: LruMap<Address, Option<Account>>,
    storage_cache: LruMap<Address, Storage>,
    bytecode_cache: LruMap<H256, Bytecode>,
    metrics: LruStateCacheMetrics,
}

impl LruStateCache {
    /// Create a new execution cache with the given capacities.
    pub fn new(
        account_cache_size: usize,
        storage_cache_size: usize,
        bytecode_cache_size: usize,
    ) -> Self {
        Self {
            account_cache: LruMap::new(ByLength::new(account_cache_size as u32)),
            storage_cache: LruMap::new(ByLength::new(storage_cache_size as u32)),
            bytecode_cache: LruMap::new(ByLength::new(bytecode_cache_size as u32)),
            metrics: LruStateCacheMetrics::default(),
        }
    }
}

impl StateCache for LruStateCache {
    fn basic_account(&mut self, address: Address) -> Option<Option<Account>> {
        let account = self.account_cache.get(&address).cloned();
        if account.is_none() {
            self.metrics.account_misses.increment(1);
        } else {
            self.metrics.account_hits.increment(1);
        }
        account
    }

    fn storage(&mut self, account: Address, storage_key: StorageKey) -> Option<StorageValue> {
        let storage = self.storage_cache.get(&account);

        if let Some(storage) = storage {
            let value = storage.storage.get(&storage_key.into()).copied();
            if value.is_some() {
                self.metrics.storage_hits.increment(1);
                value
            } else if storage.wiped {
                self.metrics.storage_hits.increment(1);
                Some(U256::ZERO)
            } else {
                self.metrics.storage_misses.increment(1);
                None
            }
        } else {
            self.metrics.storage_misses.increment(1);
            None
        }
    }

    fn bytecode_by_hash(&mut self, code_hash: H256) -> Option<Bytecode> {
        let bytecode = self.bytecode_cache.get(&code_hash).cloned();
        if bytecode.is_some() {
            self.metrics.bytecode_hits.increment(1);
        } else {
            self.metrics.bytecode_misses.increment(1);
        }
        bytecode
    }

    fn change_account(&mut self, address: Address, new: Option<Account>) {
        self.account_cache.insert(address, new);
    }

    fn change_storage(&mut self, address: Address, changes: Storage) {
        if let Some(cached_storage) = self.storage_cache.peek_mut(&address) {
            if changes.wiped {
                cached_storage.wiped = true;
                cached_storage.storage.clear();
            }

            cached_storage.storage.extend(changes.storage);
        } else {
            self.storage_cache.insert(address, changes);
        }
    }

    fn insert_bytecode(&mut self, code_hash: H256, bytecode: Bytecode) {
        self.bytecode_cache.insert(code_hash, bytecode);
    }
}

/// A cache for EVM state.
pub trait StateCache: Send + Sync {
    /// Get the basic information of an account.
    fn basic_account(&mut self, address: Address) -> Option<Option<Account>>;

    /// Get storage of given account.
    fn storage(&mut self, account: Address, storage_key: StorageKey) -> Option<StorageValue>;

    /// Get account code by its hash
    fn bytecode_by_hash(&mut self, code_hash: H256) -> Option<Bytecode>;

    /// Change the entry for an account in the cache.
    ///
    /// # Note
    ///
    /// The expected behavior here is that the entry is upserted.
    fn change_account(&mut self, address: Address, new: Option<Account>);

    /// Change the entry for the storage of an account in the cache.
    ///
    /// # Note
    ///
    /// The expected behavior here is that the entry is upserted.
    ///
    /// If the storage was wiped, the storage is wiped in the cache before being inserted.
    ///
    /// The slots in the given storage are overlaid on top of the existing storage in the cache.
    fn change_storage(&mut self, address: Address, changes: Storage);

    /// Change the entry for a bytecode in the cache.
    ///
    /// # Note
    ///
    /// The expected behavior here is that the entry is upserted.
    fn insert_bytecode(&mut self, code_hash: H256, bytecode: Bytecode);
}

/// A state provider that acts as a cache layer over another state provider.
///
/// It is important to update the cache to ensure that the state in the cache is consistent with
/// what has been (or will be) committed to the database.
pub struct CachedStateProvider<SP> {
    /// The cache.
    pub cache: Arc<Mutex<dyn StateCache>>,
    sp: SP,
}

impl<SP> CachedStateProvider<SP>
where
    SP: StateProvider,
{
    /// Create a new cached state provider using the given `cache` and inner state provider.
    pub fn new<C: StateCache + 'static>(cache: Arc<Mutex<C>>, sp: SP) -> Self {
        Self { cache, sp }
    }
}

impl<SP> BlockHashProvider for CachedStateProvider<SP>
where
    SP: StateProvider,
{
    fn block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        self.sp.block_hash(number)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        self.sp.canonical_hashes_range(start, end)
    }
}

impl<SP> AccountProvider for CachedStateProvider<SP>
where
    SP: StateProvider,
{
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(account) = cache.basic_account(address) {
            Ok(account)
        } else {
            let fetched = self.sp.basic_account(address)?;
            cache.change_account(address, fetched);
            Ok(fetched)
        }
    }
}

impl<SP> StateRootProvider for CachedStateProvider<SP>
where
    SP: StateProvider,
{
    fn state_root(&self, post_state: PostState) -> Result<H256> {
        self.sp.state_root(post_state)
    }
}

impl<SP> StateProvider for CachedStateProvider<SP>
where
    SP: StateProvider,
{
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(value) = cache.storage(account, storage_key) {
            Ok(Some(value))
        } else {
            let fetched = self.sp.storage(account, storage_key)?;
            cache.change_storage(
                account,
                Storage {
                    storage: BTreeMap::from([(storage_key.into(), fetched.unwrap_or_default())]),
                    wiped: false,
                },
            );
            Ok(fetched)
        }
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(bytecode) = cache.bytecode_by_hash(code_hash) {
            Ok(Some(bytecode))
        } else {
            let fetched = self.sp.bytecode_by_hash(code_hash)?;
            if let Some(bytecode) = fetched.clone() {
                cache.insert_bytecode(code_hash, bytecode);
            } else {
                cache.insert_bytecode(
                    code_hash,
                    Bytecode(reth_revm_primitives::primitives::Bytecode::new()),
                )
            }
            Ok(fetched)
        }
    }

    fn proof(
        &self,
        address: Address,
        keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        self.sp.proof(address, keys)
    }
}

/// LRU cache metrics
#[derive(Metrics)]
#[metrics(scope = "state.cache.lru")]
struct LruStateCacheMetrics {
    /// The number of accounts that were present in the cache
    account_hits: Counter,
    /// The number of accounts that were not present in the cache
    account_misses: Counter,
    /// The number of storage slots that were present in the cache
    storage_hits: Counter,
    /// The number of storage slots that were not present in the cache
    storage_misses: Counter,
    /// The number of bytecodes that were present in the cache
    bytecode_hits: Counter,
    /// The number of bytecodes that were not present in the cache
    bytecode_misses: Counter,
}
