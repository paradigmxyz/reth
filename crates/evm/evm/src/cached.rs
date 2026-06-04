//! Database adapters for payload building.

use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap, U256Map},
    Address, Bytes, B256, U256,
};

/// Cached bytecode for payload pre-cache bookkeeping.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Bytecode(Bytes);

impl Bytecode {
    /// Creates cached bytecode from raw bytes.
    pub const fn new_raw(bytes: Bytes) -> Self {
        Self(bytes)
    }

    /// Returns the original bytecode bytes.
    pub fn original_bytes(&self) -> Bytes {
        self.0.clone()
    }
}

/// Cached account information for payload pre-cache bookkeeping.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AccountInfo {
    /// Account balance.
    pub balance: U256,
    /// Account nonce.
    pub nonce: u64,
    /// Account code hash.
    pub code_hash: B256,
    /// Optional account id retained for compatibility with the previous cache shape.
    pub account_id: Option<u64>,
    /// Optional cached bytecode.
    pub code: Option<Bytecode>,
}

/// A container type that caches reads for payload pre-cache bookkeeping.
#[derive(Debug, Clone, Default)]
pub struct CachedReads {
    /// Block state account with storage.
    pub accounts: AddressMap<CachedAccount>,
    /// Created contracts.
    pub contracts: B256Map<Bytecode>,
    /// Block hash mapped to the block number.
    pub block_hashes: HashMap<u64, B256>,
}

// === impl CachedReads ===

impl CachedReads {
    /// Gets a placeholder database wrapper for parked legacy executor call sites.
    pub const fn as_db<DB>(&mut self, db: DB) -> CachedReadsDb<'_, DB> {
        CachedReadsDb { cached: self, db }
    }

    /// Gets a placeholder mutable database wrapper for parked legacy executor call sites.
    pub const fn as_db_mut<DB>(&mut self, db: DB) -> CachedReadsDbMut<'_, DB> {
        CachedReadsDbMut { cached: self, db }
    }

    /// Inserts an account info into the cache.
    pub fn insert_account(&mut self, address: Address, info: AccountInfo, storage: U256Map<U256>) {
        self.accounts.insert(address, CachedAccount { info: Some(info), storage });
    }

    /// Extends current cache with entries from another [`CachedReads`] instance.
    ///
    /// Note: It is expected that both instances are based on the exact same state.
    pub fn extend(&mut self, other: Self) {
        self.accounts.extend(other.accounts);
        self.contracts.extend(other.contracts);
        self.block_hashes.extend(other.block_hashes);
    }
}

/// Placeholder cache wrapper retained for parked legacy executor call sites.
#[derive(Debug)]
pub struct CachedReadsDb<'a, DB> {
    /// The cache of reads.
    pub cached: &'a mut CachedReads,
    /// The underlying database.
    pub db: DB,
}

/// Placeholder mutable cache wrapper retained for parked legacy executor call sites.
#[derive(Debug)]
pub struct CachedReadsDbMut<'a, DB> {
    /// The cache of reads.
    pub cached: &'a mut CachedReads,
    /// The underlying database.
    pub db: DB,
}

impl<'a, DB> CachedReadsDbMut<'a, DB> {
    /// Converts this mutable wrapper into an immutable placeholder wrapper.
    pub fn into_db(self) -> CachedReadsDb<'a, DB> {
        CachedReadsDb { cached: self.cached, db: self.db }
    }

    /// Returns access to wrapped database.
    pub const fn inner(&self) -> &DB {
        &self.db
    }
}

impl<DB, T> AsRef<T> for CachedReadsDbMut<'_, DB>
where
    DB: AsRef<T>,
{
    fn as_ref(&self) -> &T {
        self.inner().as_ref()
    }
}

/// Cached account contains the account state with storage but lacks the account status.
#[derive(Debug, Clone)]
pub struct CachedAccount {
    /// Account state.
    pub info: Option<AccountInfo>,
    /// Account's storage.
    pub storage: U256Map<U256>,
}

impl CachedAccount {
    /// Creates a cached account with no storage slots.
    pub fn new(info: Option<AccountInfo>) -> Self {
        Self { info, storage: U256Map::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extend_with_two_cached_reads() {
        let hash1 = B256::from_slice(&[1u8; 32]);
        let hash2 = B256::from_slice(&[2u8; 32]);
        let address1 = Address::from_slice(&[1u8; 20]);
        let address2 = Address::from_slice(&[2u8; 20]);

        let mut primary = {
            let mut cache = CachedReads::default();
            cache.accounts.insert(address1, CachedAccount::new(Some(AccountInfo::default())));
            cache.contracts.insert(hash1, Bytecode::default());
            cache.block_hashes.insert(1, hash1);
            cache
        };

        let additional = {
            let mut cache = CachedReads::default();
            cache.accounts.insert(address2, CachedAccount::new(Some(AccountInfo::default())));
            cache.contracts.insert(hash2, Bytecode::default());
            cache.block_hashes.insert(2, hash2);
            cache
        };

        primary.extend(additional);

        assert_eq!(primary.accounts.len(), 2);
        assert_eq!(primary.contracts.len(), 2);
        assert_eq!(primary.block_hashes.len(), 2);
        assert!(primary.accounts.contains_key(&address1));
        assert!(primary.accounts.contains_key(&address2));
        assert!(primary.contracts.contains_key(&hash1));
        assert!(primary.contracts.contains_key(&hash2));
        assert_eq!(primary.block_hashes.get(&1), Some(&hash1));
        assert_eq!(primary.block_hashes.get(&2), Some(&hash2));
    }
}
