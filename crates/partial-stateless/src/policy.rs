//! Cache eviction policy trait and implementations.
//!
//! Accounts and Storage/Codes can have different policies applied independently.

use crate::CachedEntry;
use alloy_primitives::{Address, Bytes, B256, U256};
use std::collections::HashMap;

/// A cache eviction policy that can be applied independently to accounts and storage/codes.
pub trait CachePolicy: Send + Sync {
    /// Evict accounts that no longer satisfy the policy.
    fn evict_accounts(
        &self,
        accounts: &mut HashMap<Address, CachedEntry<AccountData>>,
        current_block: u64,
    );

    /// Evict storage slots and bytecodes that no longer satisfy the policy.
    fn evict_storage(
        &self,
        storage: &mut HashMap<(Address, B256), CachedEntry<U256>>,
        codes: &mut HashMap<B256, CachedEntry<Bytes>>,
        current_block: u64,
    );

    /// Human-readable name of the policy (for logging/stats).
    fn name(&self) -> &str;
}

/// Account data stored in the cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountData {
    pub nonce: u64,
    pub balance: U256,
    pub code_hash: Option<B256>,
}

/// Simplest policy: retain only state accessed within the last N blocks.
///
/// Can use different `window_size` for accounts vs storage by creating
/// two separate instances.
#[derive(Debug, Clone)]
pub struct LastNBlocksPolicy {
    /// Number of blocks to retain. State not accessed within this window is evicted.
    pub window_size: u64,
}

impl LastNBlocksPolicy {
    pub fn new(window_size: u64) -> Self {
        Self { window_size }
    }
}

impl CachePolicy for LastNBlocksPolicy {
    fn evict_accounts(
        &self,
        accounts: &mut HashMap<Address, CachedEntry<AccountData>>,
        current_block: u64,
    ) {
        let cutoff = current_block.saturating_sub(self.window_size);
        accounts.retain(|_, entry| entry.last_accessed_block >= cutoff);
    }

    fn evict_storage(
        &self,
        storage: &mut HashMap<(Address, B256), CachedEntry<U256>>,
        codes: &mut HashMap<B256, CachedEntry<Bytes>>,
        current_block: u64,
    ) {
        let cutoff = current_block.saturating_sub(self.window_size);
        storage.retain(|_, entry| entry.last_accessed_block >= cutoff);
        codes.retain(|_, entry| entry.last_accessed_block >= cutoff);
    }

    fn name(&self) -> &str {
        "LastNBlocks"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_n_blocks_eviction() {
        let policy = LastNBlocksPolicy::new(10);
        let mut accounts = HashMap::new();

        // Insert an account accessed at block 5
        accounts.insert(
            Address::ZERO,
            CachedEntry {
                value: AccountData { nonce: 1, balance: U256::from(100), code_hash: None },
                first_accessed_block: 5,
                last_accessed_block: 5,
                access_count: 1,
            },
        );

        // At block 14, it should still be retained (cutoff = 4)
        policy.evict_accounts(&mut accounts, 14);
        assert_eq!(accounts.len(), 1);

        // At block 16, cutoff = 6, so block 5 entry is evicted
        policy.evict_accounts(&mut accounts, 16);
        assert_eq!(accounts.len(), 0);
    }

    #[test]
    fn test_separate_policies_different_windows() {
        let account_policy = LastNBlocksPolicy::new(20); // keep accounts longer
        let storage_policy = LastNBlocksPolicy::new(5); // evict storage faster

        let mut accounts = HashMap::new();
        let mut storage = HashMap::new();
        let mut codes = HashMap::new();

        let addr = Address::ZERO;
        let slot = B256::ZERO;

        accounts.insert(
            addr,
            CachedEntry {
                value: AccountData { nonce: 0, balance: U256::ZERO, code_hash: None },
                first_accessed_block: 10,
                last_accessed_block: 10,
                access_count: 1,
            },
        );

        storage.insert(
            (addr, slot),
            CachedEntry {
                value: U256::from(42),
                first_accessed_block: 10,
                last_accessed_block: 10,
                access_count: 1,
            },
        );

        // At block 20: account cutoff=0 (retained), storage cutoff=15 (evicted!)
        account_policy.evict_accounts(&mut accounts, 20);
        storage_policy.evict_storage(&mut storage, &mut codes, 20);

        assert_eq!(accounts.len(), 1, "account should be retained with window=20");
        assert_eq!(storage.len(), 0, "storage should be evicted with window=5");
    }
}
