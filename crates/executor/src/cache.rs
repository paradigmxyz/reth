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

use metrics::Counter;
use reth_interfaces::Result;
use reth_metrics_derive::Metrics;
use reth_primitives::{Account, Address, Bytecode, StorageKey, StorageValue, H256, U256};
use reth_provider::post_state::Storage;
use std::collections::HashMap;
use wtinylfu::WTinyLfuCache;

/// An executor-agnostic cache for EVM objects backed by multiple W-TinyLFU caches.
///
/// See the [module level][super] docs for more information.
pub struct ExecutionCache {
    account_cache: WTinyLfuCache<Address, Option<Account>>,
    storage_cache: WTinyLfuCache<Address, Storage>,
    bytecode_cache: WTinyLfuCache<H256, Option<Bytecode>>,
    metrics: ExecutionCacheMetrics,
}

// TODO: Replace bytecode cache with an LRU cache (no need for it to be complicated)
// TODO: Figure out sample sizes
// TODO: Appropriate default cache sizes
// TODO: On sample sizes/cache mechanism: https://9vx.org/post/on-window-tinylfu/
// TODO: https://news.ycombinator.com/item?id=26452890
// TODO: Explain current sample sizes and why we chose them
// TODO: A way to commit changes

impl ExecutionCache {
    /// Create a new execution cache with the given capacities.
    pub fn new(
        account_cache_size: usize,
        storage_cache_size: usize,
        bytecode_cache_size: usize,
    ) -> Self {
        Self {
            account_cache: WTinyLfuCache::new(account_cache_size, account_cache_size * 16),
            storage_cache: WTinyLfuCache::new(storage_cache_size, account_cache_size * 16),
            bytecode_cache: WTinyLfuCache::new(bytecode_cache_size, bytecode_cache_size * 8),
            metrics: ExecutionCacheMetrics::default(),
        }
    }

    /// Returns a clone of the account corresponding to the given address.
    ///
    /// If the account is not in the cache, calls `f`.
    ///
    /// # Note
    ///
    /// This produces a cache sample, updates metrics, and bumps recency of the entry.
    pub fn get_account_with<F>(&mut self, address: Address, f: F) -> Result<Option<Account>>
    where
        F: FnOnce() -> Result<Option<Account>>,
    {
        Ok(match self.account_cache.get(&address) {
            Some(entry) => {
                self.metrics.account_hits.increment(1);
                entry.clone()
            }
            None => {
                self.metrics.account_misses.increment(1);
                let account = (f)()?;
                if self.account_cache.push(address, account.clone()).is_some() {
                    self.metrics.account_evictions.increment(1);
                }
                account
            }
        })
    }

    /// Returns a reference to the account corresponding to the given address.
    ///
    /// # Note
    ///
    /// This does not produce a cache sample, update metrics, or increase recency of the entry.
    pub fn get_account(&mut self, address: Address) -> Option<&Account> {
        if let Some(account) = self.account_cache.get(&address).map(|a| a.as_ref()) {
            account
        } else {
            None
        }
    }

    /// Returns a clone of the storage value corresponding to the given storage slot.
    ///
    /// If the value is not in the cache, calls `f`.
    ///
    /// # Note
    ///
    /// This produces a cache sample, updates metrics, and bumps recency of the entry.
    pub fn get_storage_with<F>(
        &mut self,
        address: Address,
        slot: StorageKey,
        f: F,
    ) -> Result<StorageValue>
    where
        F: FnOnce() -> Result<StorageValue>,
    {
        let storage = self.storage_cache.get_mut(&address);

        if let Some(storage) = storage {
            if let Some(value) = storage.storage.get(&slot.into()) {
                self.metrics.storage_hits.increment(1);
                Ok(*value)
            } else if storage.wiped {
                // NOTE: This counts as a hit since we know the storage value is 0.
                self.metrics.storage_hits.increment(1);
                let value = StorageValue::ZERO;
                let _ = storage.storage.insert(slot.into(), value.into());
                Ok(value)
            } else {
                self.metrics.storage_misses.increment(1);
                let value = (f)()?;
                let _ = storage.storage.insert(slot.into(), value.into());
                Ok(value)
            }
        } else {
            self.metrics.storage_misses.increment(1);
            let value = (f)()?;
            if let Some((_, storage)) = self.storage_cache.push(
                address,
                Storage {
                    storage: HashMap::from([(U256::from_be_bytes(slot.to_fixed_bytes()), value)]),
                    wiped: false,
                },
            ) {
                self.metrics.storage_evictions.increment(storage.storage.len() as u64);
            }
            Ok(value)
        }
    }

    /// Returns a clone of the bytecode corresponding to the given code hash.
    ///
    /// If the bytecode is not in the cache, calls `f`.
    ///
    /// # Note
    ///
    /// This produces a cache sample, updates metrics, and bumps recency of the entry.
    pub fn get_bytecode_with<F>(&mut self, code_hash: H256, f: F) -> Result<Option<Bytecode>>
    where
        F: FnOnce() -> Result<Option<Bytecode>>,
    {
        Ok(match self.bytecode_cache.get(&code_hash) {
            Some(bytecode) => {
                self.metrics.bytecode_hits.increment(1);
                bytecode.clone()
            }
            None => {
                self.metrics.bytecode_misses.increment(1);
                let bytecode = (f)()?;
                if self.bytecode_cache.push(code_hash, bytecode.clone()).is_some() {
                    self.metrics.bytecode_evictions.increment(1);
                }
                bytecode
            }
        })
    }

    /// Returns a reference to the bytecode corresponding to the given code hash.
    ///
    /// # Note
    ///
    /// This does not produce a cache sample, update metrics, or increase recency of the entry.
    pub fn get_bytecode(&mut self, code_hash: H256) -> Option<Bytecode> {
        self.bytecode_cache.get(&code_hash).cloned().flatten()
    }

    /// Clear the storage of an account.
    pub fn clear_storage(&mut self, address: Address) {
        if let Some(cached_storage) = self.storage_cache.peek_mut(&address) {
            cached_storage.storage.clear();
            cached_storage.wiped = true;
        }
    }

    /// Update the storage of an account.
    pub fn update_storage<T: IntoIterator<Item = (U256, U256)>>(
        &mut self,
        address: Address,
        iter: T,
    ) {
        if let Some(cached_storage) = self.storage_cache.get_mut(&address) {
            cached_storage.storage.extend(iter);
        } else {
            self.storage_cache
                .push(address, Storage { storage: iter.into_iter().collect(), wiped: false });
        }
    }

    /// Update an account.
    pub fn update_account(&mut self, address: Address, account: Option<Account>) {
        self.account_cache.push(address, account.clone());
    }

    /// Update a bytecode.
    pub fn update_bytecode(&mut self, code_hash: H256, code: Bytecode) {
        self.bytecode_cache.push(code_hash, Some(code));
    }
}

/// Execution cache metrics
#[derive(Metrics)]
#[metrics(scope = "sync.execution.cache")]
pub struct ExecutionCacheMetrics {
    /// The number of accounts that were present in the cache
    account_hits: Counter,
    /// The number of accounts that were not present in the cache
    account_misses: Counter,
    /// The number of accounts evicted from the cache
    account_evictions: Counter,
    /// The number of storage slots that were present in the cache
    storage_hits: Counter,
    /// The number of storage slots that were not present in the cache
    storage_misses: Counter,
    /// The number of storage slots evicted from the cache
    storage_evictions: Counter,
    /// The number of bytecodes that were present in the cache
    bytecode_hits: Counter,
    /// The number of bytecodes that were not present in the cache
    bytecode_misses: Counter,
    /// The number of bytecodes evicted from the cache
    bytecode_evictions: Counter,
}
