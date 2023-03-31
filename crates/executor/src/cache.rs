//! Long lived execution cache.
//!
//! This cache stores accounts, bytecodes, and storage slots during its lifetime. It is intended to
//! live for as long as possible in order to reduce the number of reads from the database.
//!
//! The execution cache itself ([`ExecutionCache`]) is just a store: in order to use it with an
//! executor, it must be constructed into a state provider by composing it on top of another state
//! provider.
//!
//! You must commit changes to the state to the cache after each transaction, otherwise the executor
//! will be using outdated state.
//!
//! # Using The Cache
//!
//! TODO: Example
//!
//! # Approximating Memory Usage
//!
//! TODO: What is a good cache size? How much memory does it use?
//! TODO: Figure out if we should store storage and accounts separately
//!
//! # Cache Policy
//!
//! The cache itself wraps a W-TinyLFU based cache. W-TinyLFU is good for temporal access patterns
//! over periods of time. The W-TinyLFU paper is accessible [on arXiv][w-tinylfu-paper].
//!
//! [w-tinylfu-paper]: https://arxiv.org/abs/1512.00727

use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, H256, U256,
};
use reth_provider::{post_state::Storage, AccountProvider, BlockHashProvider, StateProvider};
use revm::precompile::HashMap;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tracing::trace;
use wtinylfu::WTinyLfuCache;

// TODO: Impl debug

pub struct ExecutionCache {
    account_cache: WTinyLfuCache<Address, Option<Account>>,
    storage_cache: WTinyLfuCache<Address, BTreeMap<U256, U256>>,
    bytecode_cache: WTinyLfuCache<H256, Option<Bytecode>>,
    // TODO: Temp
    pub account_hits: u64,
    pub account_misses: u64,
    pub account_evictions: u64,
    pub storage_hits: u64,
    pub storage_misses: u64,
    pub storage_evictions: u64,
    pub bytecode_hits: u64,
    pub bytecode_misses: u64,
    pub bytecode_evictions: u64,
}

// TODO: Replace bytecode cache with an LRU cache (no need for it to be complicated)
// TODO: Figure out sample sizes
// TODO: Appropriate default cache sizes
// TODO: return Result<Option>

impl ExecutionCache {
    // TODO: On sample sizes/cache mechanism: https://9vx.org/post/on-window-tinylfu/
    // TODO: https://news.ycombinator.com/item?id=26452890
    pub fn new(
        account_cache_size: usize,
        storage_cache_size: usize,
        bytecode_cache_size: usize,
    ) -> Self {
        Self {
            account_cache: WTinyLfuCache::new(account_cache_size, account_cache_size * 16),
            storage_cache: WTinyLfuCache::new(storage_cache_size, account_cache_size * 16),
            bytecode_cache: WTinyLfuCache::new(bytecode_cache_size, bytecode_cache_size * 8),
            account_hits: 0,
            account_misses: 0,
            account_evictions: 0,
            storage_hits: 0,
            storage_misses: 0,
            storage_evictions: 0,
            bytecode_hits: 0,
            bytecode_misses: 0,
            bytecode_evictions: 0,
        }
    }

    pub fn get_account_with<F>(&mut self, address: Address, f: F) -> Option<Account>
    where
        F: FnOnce() -> Option<Account>,
    {
        match self.account_cache.get(&address) {
            Some(entry) => {
                self.account_hits += 1;
                entry.clone()
            }
            None => {
                self.account_misses += 1;
                let account = (f)();
                if self.account_cache.push(address, account.clone()).is_some() {
                    self.account_evictions += 1;
                }
                account
            }
        }
    }

    pub fn get_storage_with<F>(
        &mut self,
        address: Address,
        slot: StorageKey,
        f: F,
    ) -> Option<StorageValue>
    where
        F: FnOnce() -> Option<StorageValue>,
    {
        let storage = self.storage_cache.get_mut(&address);

        if let Some(storage) = storage {
            if let Some(value) = storage.get(&slot.into()) {
                self.storage_hits += 1;
                Some(*value)
            } else {
                self.storage_misses += 1;
                let value = (f)().unwrap_or_default();
                let _ = storage.insert(slot.into(), value.into());
                Some(value)
            }
        } else {
            self.storage_hits += 1;
            let value = (f)().unwrap_or_default();
            if self
                .storage_cache
                .push(address, BTreeMap::from([(slot.into(), value.into())]))
                .is_some()
            {
                self.storage_evictions += 1;
            }
            Some(value)
        }
    }

    pub fn get_bytecode_with<F>(&mut self, code_hash: H256, f: F) -> Option<Bytecode>
    where
        F: FnOnce() -> Option<Bytecode>,
    {
        match self.bytecode_cache.get(&code_hash) {
            Some(bytecode) => {
                self.bytecode_hits += 1;
                bytecode.clone()
            }
            None => {
                self.bytecode_misses += 1;
                let bytecode = (f)();
                if self.bytecode_cache.push(code_hash, bytecode.clone()).is_some() {
                    self.bytecode_evictions += 1;
                }
                bytecode
            }
        }
    }

    // TODO: Note why we only update existing values
    pub fn update_cache(
        &mut self,
        accounts: &BTreeMap<Address, Option<Account>>,
        storage: &BTreeMap<Address, Storage>,
    ) {
        for (address, account) in accounts {
            if let Some(entry) = self.account_cache.peek_mut(address) {
                *entry = account.clone();
            }
        }

        for (address, storage) in storage {
            if let Some(cached_storage) = self.storage_cache.peek_mut(address) {
                if storage.wiped {
                    cached_storage.clear();
                }
                cached_storage.extend(storage.storage.clone());
            }
        }
    }

    // TODO: A way to commit changes
    // TODO: We need to keep a cache DB for diffing...
}

pub struct CachedStateProvider<SP> {
    cache: Arc<Mutex<ExecutionCache>>,
    /// The inner state provider.
    provider: SP,
}

impl<'a, SP: StateProvider> CachedStateProvider<SP> {
    pub fn new(cache: Arc<Mutex<ExecutionCache>>, provider: SP) -> Self {
        Self { cache, provider }
    }
}

impl<'a, SP: StateProvider> BlockHashProvider for CachedStateProvider<SP> {
    fn block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>> {
        self.provider.block_hash(block_number)
    }

    fn canonical_hashes_range(&self, _start: BlockNumber, _end: BlockNumber) -> Result<Vec<H256>> {
        unimplemented!()
    }
}

impl<'a, SP: StateProvider> AccountProvider for CachedStateProvider<SP> {
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        Ok(self
            .cache
            .lock()
            .unwrap()
            .get_account_with(address, || self.provider.basic_account(address).ok().flatten()))
    }
}

impl<'a, SP: StateProvider> StateProvider for CachedStateProvider<SP> {
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        Ok(self.cache.lock().unwrap().get_storage_with(account, storage_key, || {
            self.provider.storage(account, storage_key).ok().flatten()
        }))
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        Ok(self.cache.lock().unwrap().get_bytecode_with(code_hash, || {
            self.provider.bytecode_by_hash(code_hash).ok().flatten()
        }))
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::HistoryStateRoot.into())
    }
}
