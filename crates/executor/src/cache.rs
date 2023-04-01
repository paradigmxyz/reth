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
use reth_provider::post_state::Storage;
use std::{
    collections::{BTreeMap, HashMap},
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

    pub fn get_account_with<F>(&mut self, address: Address, f: F) -> Result<Option<Account>>
    where
        F: FnOnce() -> Result<Option<Account>>,
    {
        Ok(match self.account_cache.get(&address) {
            Some(entry) => {
                self.account_hits += 1;
                entry.clone()
            }
            None => {
                self.account_misses += 1;
                let account = (f)()?;
                if self.account_cache.push(address, account.clone()).is_some() {
                    self.account_evictions += 1;
                }
                account
            }
        })
    }

    // TODO
    pub fn get_account(&mut self, address: Address) -> Option<&Account> {
        if let Some(account) = self.account_cache.get(&address).map(|a| a.as_ref()) {
            account
        } else {
            None
        }
    }

    pub fn get_storage_with<F>(
        &mut self,
        address: Address,
        slot: StorageKey,
        f: F,
    ) -> Result<StorageValue>
    where
        F: FnOnce() -> Result<StorageValue>,
    {
        // TODO: If storage = wiped, don't ask DB
        // TODO: New account should have storage = wiped
        let storage = self.storage_cache.get_mut(&address);

        if let Some(storage) = storage {
            if let Some(value) = storage.get(&slot.into()) {
                self.storage_hits += 1;
                Ok(*value)
            } else {
                self.storage_misses += 1;
                let value = (f)()?;
                let _ = storage.insert(slot.into(), value.into());
                Ok(value)
            }
        } else {
            self.storage_hits += 1;
            let value = (f)()?;
            if self
                .storage_cache
                .push(address, BTreeMap::from([(slot.into(), value.into())]))
                .is_some()
            {
                self.storage_evictions += 1;
            }
            Ok(value)
        }
    }

    pub fn get_bytecode_with<F>(&mut self, code_hash: H256, f: F) -> Result<Option<Bytecode>>
    where
        F: FnOnce() -> Result<Option<Bytecode>>,
    {
        Ok(match self.bytecode_cache.get(&code_hash) {
            Some(bytecode) => {
                self.bytecode_hits += 1;
                bytecode.clone()
            }
            None => {
                self.bytecode_misses += 1;
                let bytecode = (f)()?;
                if self.bytecode_cache.push(code_hash, bytecode.clone()).is_some() {
                    self.bytecode_evictions += 1;
                }
                bytecode
            }
        })
    }

    // TODO
    pub fn get_bytecode(&mut self, code_hash: H256) -> Option<Bytecode> {
        self.bytecode_cache.get(&code_hash).cloned().flatten()
    }

    #[deprecated]
    pub fn clear_storage(&mut self, address: Address) {
        if let Some(cached_storage) = self.storage_cache.peek_mut(&address) {
            cached_storage.clear();
        }
    }

    #[deprecated]
    pub fn update_storage<T: IntoIterator<Item = (U256, U256)>>(
        &mut self,
        address: Address,
        iter: T,
    ) {
        if let Some(cached_storage) = self.storage_cache.get_mut(&address) {
            cached_storage.extend(iter);
        } else {
            self.storage_cache.push(address, iter.into_iter().collect());
        }
    }

    #[deprecated]
    pub fn update_account(&mut self, address: Address, account: Option<Account>) {
        self.account_cache.push(address, account.clone());
    }

    #[deprecated]
    pub fn update_bytecode(&mut self, code_hash: H256, code: Bytecode) {
        self.bytecode_cache.push(code_hash, Some(code));
    }

    // TODO: Temp
    #[deprecated]
    pub fn drop_account(&mut self, address: Address) {
        self.account_cache.pop(&address);
        self.storage_cache.pop(&address);
    }

    // TODO: A way to commit changes
    // TODO: We need to keep a cache DB for diffing...
}
