//! Immutable snapshots produced by txpool-driven state prewarming.

use alloy_primitives::{Address, StorageKey, StorageValue, B256, U256};
use reth_primitives_traits::{Account, Bytecode};
use reth_revm::cached::CachedReads;
use std::sync::Arc;

/// A deep, immutable txpool-prewarm cache snapshot for one parent state.
///
/// Wraps the [`CachedReads`] collected by speculative execution so cache tiers can serve lookups
/// from it directly.
#[derive(Clone, Debug)]
pub struct TxPoolPrewarmCacheSnapshot {
    parent_hash: B256,
    reads: Arc<CachedReads>,
}

impl TxPoolPrewarmCacheSnapshot {
    /// Creates a snapshot of `reads` collected against `parent_hash`'s state.
    pub const fn new(parent_hash: B256, reads: Arc<CachedReads>) -> Self {
        Self { parent_hash, reads }
    }

    /// Returns the hash of the state this snapshot was warmed against.
    pub const fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    /// Returns a cached account, preserving cached non-existence.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.reads.accounts.get(address).map(|account| account.info.as_ref().map(Account::from))
    }

    /// Returns a cached storage value, preserving cached zero values.
    pub fn storage(&self, address: Address, key: StorageKey) -> Option<StorageValue> {
        self.reads.accounts.get(&address)?.storage.get(&U256::from_be_bytes(key.0)).copied()
    }

    /// Returns cached bytecode.
    ///
    /// [`CachedReads`] stores empty bytecode for code it could not find, so an empty entry reads
    /// as a miss and the caller's fallback tier decides.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Option<Bytecode>> {
        let code = self.reads.contracts.get(code_hash)?;
        (!code.is_empty()).then(|| Some(Bytecode(code.clone())))
    }

    /// Returns `(accounts, storage slots, bytecodes)` in the snapshot.
    pub fn entry_counts(&self) -> (usize, usize, usize) {
        (
            self.reads.accounts.len(),
            self.reads.accounts.values().map(|account| account.storage.len()).sum(),
            self.reads.contracts.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::U256Map;
    use reth_revm::{
        cached::CachedAccount,
        revm::{bytecode::Bytecode as RevmBytecode, state::AccountInfo},
    };

    #[test]
    fn lookups_preserve_cache_semantics() {
        let owner = Address::repeat_byte(0x01);
        let missing = Address::repeat_byte(0x02);
        let code_hash = B256::repeat_byte(0x0C);
        let empty_code_hash = B256::repeat_byte(0x0D);

        let mut reads = CachedReads::default();
        let mut storage = U256Map::default();
        storage.insert(U256::from(1), U256::from(7));
        storage.insert(U256::from(2), U256::ZERO);
        reads.insert_account(owner, AccountInfo { nonce: 3, ..Default::default() }, storage);
        reads.accounts.insert(missing, CachedAccount { info: None, storage: Default::default() });
        reads.contracts.insert(code_hash, RevmBytecode::new_raw([0x60, 0x01].into()));
        reads.contracts.insert(empty_code_hash, RevmBytecode::default());

        let snapshot = TxPoolPrewarmCacheSnapshot::new(B256::ZERO, Arc::new(reads));

        assert_eq!(snapshot.account(&owner).unwrap().unwrap().nonce, 3);
        assert_eq!(snapshot.account(&missing), Some(None), "non-existence is a cacheable fact");
        assert_eq!(snapshot.account(&Address::repeat_byte(0x03)), None);

        let slot = |n: u64| B256::from(U256::from(n));
        assert_eq!(snapshot.storage(owner, slot(1)), Some(U256::from(7)));
        assert_eq!(snapshot.storage(owner, slot(2)), Some(U256::ZERO), "zero values are hits");
        assert_eq!(snapshot.storage(owner, slot(9)), None);
        assert_eq!(snapshot.storage(missing, slot(1)), None);

        assert!(snapshot.bytecode(&code_hash).unwrap().is_some());
        assert_eq!(snapshot.bytecode(&empty_code_hash), None, "empty code reads as a miss");
        assert_eq!(snapshot.bytecode(&B256::repeat_byte(0x0E)), None);

        assert_eq!(snapshot.entry_counts(), (2, 2, 2));
    }
}
