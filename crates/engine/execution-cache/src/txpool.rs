//! Immutable snapshots produced by txpool-driven state prewarming.

use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap},
    Address, StorageKey, StorageValue, B256,
};
use reth_primitives_traits::{Account, Bytecode};
use std::sync::Arc;

/// A deep, immutable txpool-prewarm cache snapshot for one parent state.
#[derive(Clone, Debug)]
pub struct TxPoolPrewarmCacheSnapshot {
    inner: Arc<TxPoolPrewarmCacheSnapshotInner>,
}

impl TxPoolPrewarmCacheSnapshot {
    /// Creates a snapshot from fully owned cache maps.
    pub fn from_parts(
        parent_hash: B256,
        accounts: AddressMap<Option<Account>>,
        storage: HashMap<(Address, StorageKey), StorageValue>,
        bytecodes: B256Map<Option<Bytecode>>,
    ) -> Self {
        Self {
            inner: Arc::new(TxPoolPrewarmCacheSnapshotInner {
                parent_hash,
                accounts,
                storage,
                bytecodes,
            }),
        }
    }

    /// Returns the hash of the state this snapshot was warmed against.
    pub fn parent_hash(&self) -> B256 {
        self.inner.parent_hash
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

#[derive(Debug)]
struct TxPoolPrewarmCacheSnapshotInner {
    parent_hash: B256,
    accounts: AddressMap<Option<Account>>,
    storage: HashMap<(Address, StorageKey), StorageValue>,
    bytecodes: B256Map<Option<Bytecode>>,
}
