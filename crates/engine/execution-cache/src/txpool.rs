//! Reusable cache and immutable snapshots for txpool-driven state prewarming.

use crate::CachedStatus;
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, StorageKey, StorageValue, B256,
};
use parking_lot::RwLock;
use reth_primitives_traits::{Account, Bytecode};
use std::sync::Arc;

/// A mutable state-read cache reused by the txpool prewarmer between canonical heads.
///
/// The prewarmer is the only logical writer, although its transaction workers can fill the cache
/// concurrently. [`Self::clear`] retains the maps' allocations for the next head.
#[derive(Debug, Default)]
pub struct TxPoolPrewarmCache {
    accounts: RwLock<HashMap<Address, Option<Account>>>,
    storage: RwLock<HashMap<(Address, StorageKey), StorageValue>>,
    bytecodes: RwLock<HashMap<B256, Option<Bytecode>>>,
}

impl TxPoolPrewarmCache {
    /// Creates an empty reusable cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all cached state while retaining the backing allocations.
    pub fn clear(&self) {
        self.accounts.write().clear();
        self.storage.write().clear();
        self.bytecodes.write().clear();
    }

    /// Creates a deep, immutable snapshot tagged with the state hash it was warmed against.
    ///
    /// Callers must only snapshot at a completed prewarming-wave boundary, when no workers are
    /// writing to the cache.
    pub fn snapshot(
        &self,
        parent_hash: B256,
        warmed_transactions: impl IntoIterator<Item = B256>,
    ) -> TxPoolPrewarmCacheSnapshot {
        TxPoolPrewarmCacheSnapshot {
            inner: Arc::new(TxPoolPrewarmCacheSnapshotInner {
                parent_hash,
                warmed_transactions: warmed_transactions.into_iter().collect(),
                accounts: self.accounts.read().clone(),
                storage: self.storage.read().clone(),
                bytecodes: self.bytecodes.read().clone(),
            }),
        }
    }

    /// Gets an account or fills it using `f`.
    pub fn get_or_try_insert_account_with<E>(
        &self,
        address: Address,
        f: impl FnOnce() -> Result<Option<Account>, E>,
    ) -> Result<CachedStatus<Option<Account>>, E> {
        if let Some(account) = self.accounts.read().get(&address).copied() {
            return Ok(CachedStatus::Cached(account))
        }

        let account = f()?;
        let mut accounts = self.accounts.write();
        if let Some(cached) = accounts.get(&address).copied() {
            Ok(CachedStatus::Cached(cached))
        } else {
            accounts.insert(address, account);
            Ok(CachedStatus::NotCached(account))
        }
    }

    /// Gets a storage value or fills it using `f`.
    pub fn get_or_try_insert_storage_with<E>(
        &self,
        address: Address,
        key: StorageKey,
        f: impl FnOnce() -> Result<StorageValue, E>,
    ) -> Result<CachedStatus<StorageValue>, E> {
        if let Some(value) = self.storage.read().get(&(address, key)).copied() {
            return Ok(CachedStatus::Cached(value))
        }

        let value = f()?;
        let mut storage = self.storage.write();
        if let Some(cached) = storage.get(&(address, key)).copied() {
            Ok(CachedStatus::Cached(cached))
        } else {
            storage.insert((address, key), value);
            Ok(CachedStatus::NotCached(value))
        }
    }

    /// Gets bytecode or fills it using `f`.
    pub fn get_or_try_insert_code_with<E>(
        &self,
        code_hash: B256,
        f: impl FnOnce() -> Result<Option<Bytecode>, E>,
    ) -> Result<CachedStatus<Option<Bytecode>>, E> {
        if let Some(code) = self.bytecodes.read().get(&code_hash).cloned() {
            return Ok(CachedStatus::Cached(code))
        }

        let code = f()?;
        let mut bytecodes = self.bytecodes.write();
        if let Some(cached) = bytecodes.get(&code_hash).cloned() {
            Ok(CachedStatus::Cached(cached))
        } else {
            bytecodes.insert(code_hash, code.clone());
            Ok(CachedStatus::NotCached(code))
        }
    }
}

/// A deep, immutable txpool-prewarm cache snapshot for one parent state.
#[derive(Clone, Debug)]
pub struct TxPoolPrewarmCacheSnapshot {
    inner: Arc<TxPoolPrewarmCacheSnapshotInner>,
}

#[derive(Debug)]
struct TxPoolPrewarmCacheSnapshotInner {
    parent_hash: B256,
    warmed_transactions: HashSet<B256>,
    accounts: HashMap<Address, Option<Account>>,
    storage: HashMap<(Address, StorageKey), StorageValue>,
    bytecodes: HashMap<B256, Option<Bytecode>>,
}

impl TxPoolPrewarmCacheSnapshot {
    /// Returns the hash of the state this snapshot was warmed against.
    pub fn parent_hash(&self) -> B256 {
        self.inner.parent_hash
    }

    /// Returns whether the transaction contributed to this prewarmed snapshot.
    pub fn contains_transaction(&self, transaction_hash: &B256) -> bool {
        self.inner.warmed_transactions.contains(transaction_hash)
    }

    /// Returns a cached account, preserving cached non-existence.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.inner.accounts.get(address).copied()
    }

    /// Returns a cached storage value, preserving cached zero values.
    pub fn storage(&self, address: Address, key: StorageKey) -> Option<StorageValue> {
        self.inner.storage.get(&(address, key)).copied()
    }

    /// Returns cached bytecode, preserving cached non-existence.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Option<Bytecode>> {
        self.inner.bytecodes.get(code_hash).cloned()
    }

    /// Returns `(accounts, storage slots, bytecodes)` in the snapshot.
    pub fn entry_counts(&self) -> (usize, usize, usize) {
        (self.inner.accounts.len(), self.inner.storage.len(), self.inner.bytecodes.len())
    }
}
