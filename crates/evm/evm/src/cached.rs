//! Database adapters for payload building.
use alloy_primitives::{
    map::{AddressMap, B256Map, Entry, HashMap, U256Map},
    Address, B256, U256,
};
pub use evm2::{bytecode::Bytecode, evm::AccountInfo};
use evm2::{evm::Database, interpreter::Word};

/// A container type that caches reads from an underlying [`Database`].
///
/// This is intended to be used in conjunction with block executors and builders
/// during payload building which repeatedly accesses the same data.
///
/// [`CachedReads::as_db_mut`] transforms this type into a [`Database`] implementation that uses
/// [`CachedReads`] as a caching layer for operations, and records any cache misses.
///
/// # Example
///
/// ```
/// use evm2::evm::Database;
/// use reth_evm::cached::CachedReads;
///
/// fn build_payload<DB: Database>(db: DB) {
///     let mut cached_reads = CachedReads::default();
///     let db = cached_reads.as_db_mut(db);
///     // this is `Database` and can be used to build a payload, it never commits to `CachedReads` or the underlying database, but all reads from the underlying database are cached in `CachedReads`.
///     // Subsequent payload build attempts can use cached reads and avoid hitting the underlying database.
///     // Note: `cached_reads` must outlive `db` to satisfy lifetime requirements.
/// }
/// ```
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
    /// Creates a new [`CachedReads`] with capacity for account entries.
    pub fn with_account_capacity(capacity: usize) -> Self {
        Self {
            accounts: AddressMap::with_capacity_and_hasher(capacity, Default::default()),
            ..Default::default()
        }
    }

    /// Gets a [`Database`] that will cache reads from the given database.
    pub const fn as_db<DB>(&mut self, db: DB) -> CachedReadsDbMut<'_, DB> {
        self.as_db_mut(db).into_db()
    }

    /// Gets a mutable [`Database`] that will cache reads from the underlying database.
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

/// A [Database] that caches reads inside [`CachedReads`].
///
/// The lifetime parameter `'a` is tied to the lifetime of the underlying [`CachedReads`] instance.
/// This ensures that the cache remains valid for the entire duration this wrapper is used.
/// The original [`CachedReads`] must outlive this wrapper to prevent use-after-free.
#[derive(Debug)]
pub struct CachedReadsDbMut<'a, DB> {
    /// The cache of reads.
    pub cached: &'a mut CachedReads,
    /// The underlying database.
    pub db: DB,
}

impl<'a, DB> CachedReadsDbMut<'a, DB> {
    /// Converts this [`Database`] implementation into a database wrapper that will still cache
    /// reads.
    pub const fn into_db(self) -> Self {
        self
    }

    /// Returns access to wrapped [`Database`].
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

impl<DB: Database> Database for CachedReadsDbMut<'_, DB> {
    type Error = DB::Error;

    fn is_fatal(error: &Self::Error) -> bool {
        DB::is_fatal(error)
    }

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let basic = match self.cached.accounts.entry(*address) {
            Entry::Occupied(entry) => entry.get().info.clone(),
            Entry::Vacant(entry) => {
                entry.insert(CachedAccount::new(self.db.get_account(address)?)).info.clone()
            }
        };
        Ok(basic)
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        let code = match self.cached.contracts.entry(*code_hash) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => entry.insert(self.db.get_code_by_hash(code_hash)?).clone(),
        };
        Ok(code)
    }

    fn get_storage(&mut self, address: &Address, index: &Word) -> Result<Word, Self::Error> {
        match self.cached.accounts.entry(*address) {
            Entry::Occupied(mut acc_entry) => match acc_entry.get_mut().storage.entry(*index) {
                Entry::Occupied(entry) => Ok(*entry.get()),
                Entry::Vacant(entry) => Ok(*entry.insert(self.db.get_storage(address, index)?)),
            },
            Entry::Vacant(acc_entry) => {
                // acc needs to be loaded for us to access slots.
                let info = self.db.get_account(address)?;
                let (account, value) = if info.is_some() {
                    let value = self.db.get_storage(address, index)?;
                    let mut account = CachedAccount::new(info);
                    account.storage.insert(*index, value);
                    (account, value)
                } else {
                    (CachedAccount::new(info), U256::ZERO)
                };
                acc_entry.insert(account);
                Ok(value)
            }
        }
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<B256, Self::Error> {
        let number = number.saturating_to();
        let hash = match self.cached.block_hashes.entry(number) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => *entry.insert(self.db.get_block_hash(&U256::from(number))?),
        };
        Ok(hash)
    }
}

/// Cached account contains the account state with storage
/// but lacks the account status.
#[derive(Debug, Clone)]
pub struct CachedAccount {
    /// Account state.
    pub info: Option<AccountInfo>,
    /// Account's storage.
    pub storage: U256Map<U256>,
}

impl CachedAccount {
    fn new(info: Option<AccountInfo>) -> Self {
        Self { info, storage: U256Map::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use std::{error::Error, fmt, rc::Rc};

    #[test]
    fn test_extend_with_two_cached_reads() {
        // Setup test data
        let hash1 = B256::from_slice(&[1u8; 32]);
        let hash2 = B256::from_slice(&[2u8; 32]);
        let address1 = Address::from_slice(&[1u8; 20]);
        let address2 = Address::from_slice(&[2u8; 20]);

        // Create primary cache
        let mut primary = {
            let mut cache = CachedReads::default();
            cache.accounts.insert(address1, CachedAccount::new(Some(AccountInfo::default())));
            cache.contracts.insert(hash1, Bytecode::default());
            cache.block_hashes.insert(1, hash1);
            cache
        };

        // Create additional cache
        let additional = {
            let mut cache = CachedReads::default();
            cache.accounts.insert(address2, CachedAccount::new(Some(AccountInfo::default())));
            cache.contracts.insert(hash2, Bytecode::default());
            cache.block_hashes.insert(2, hash2);
            cache
        };

        // Extending primary with additional cache
        primary.extend(additional);

        // Verify the combined state
        assert!(
            primary.accounts.len() == 2 &&
                primary.contracts.len() == 2 &&
                primary.block_hashes.len() == 2,
            "All maps should contain 2 entries"
        );

        // Verify specific entries
        assert!(
            primary.accounts.contains_key(&address1) &&
                primary.accounts.contains_key(&address2) &&
                primary.contracts.contains_key(&hash1) &&
                primary.contracts.contains_key(&hash2) &&
                primary.block_hashes.get(&1) == Some(&hash1) &&
                primary.block_hashes.get(&2) == Some(&hash2),
            "All expected entries should be present"
        );
    }

    #[derive(Clone, Debug)]
    struct CountingDb {
        account: Option<AccountInfo>,
        code: Bytecode,
        storage: Word,
        block_hash: B256,
        reads: Rc<ReadCounts>,
    }

    #[derive(Debug, Default)]
    struct ReadCounts {
        accounts: Cell<usize>,
        code: Cell<usize>,
        storage: Cell<usize>,
        block_hashes: Cell<usize>,
    }

    #[derive(Debug)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("test database error")
        }
    }

    impl Error for TestError {}

    impl Database for CountingDb {
        type Error = TestError;

        fn get_account(&mut self, _address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            self.reads.accounts.set(self.reads.accounts.get() + 1);
            Ok(self.account.clone())
        }

        fn get_code_by_hash(&mut self, _code_hash: &B256) -> Result<Bytecode, Self::Error> {
            self.reads.code.set(self.reads.code.get() + 1);
            Ok(self.code.clone())
        }

        fn get_storage(&mut self, _address: &Address, _key: &Word) -> Result<Word, Self::Error> {
            self.reads.storage.set(self.reads.storage.get() + 1);
            Ok(self.storage)
        }

        fn get_block_hash(&mut self, _number: &Word) -> Result<B256, Self::Error> {
            self.reads.block_hashes.set(self.reads.block_hashes.get() + 1);
            Ok(self.block_hash)
        }
    }

    #[test]
    fn cached_reads_database_avoids_repeated_backing_reads() {
        let address = Address::repeat_byte(1);
        let code_hash = B256::repeat_byte(2);
        let block_hash = B256::repeat_byte(3);
        let key = U256::from(4);
        let value = U256::from(5);
        let reads = Rc::new(ReadCounts::default());
        let db = CountingDb {
            account: Some(AccountInfo::new(U256::from(1), 2, code_hash, None)),
            code: Bytecode::new_raw([0x00].as_slice().into()),
            storage: value,
            block_hash,
            reads: reads.clone(),
        };

        let mut cached = CachedReads::default();
        let mut db = cached.as_db_mut(db);

        assert!(db.get_account(&address).unwrap().is_some());
        assert!(db.get_account(&address).unwrap().is_some());
        assert_eq!(db.get_code_by_hash(&code_hash).unwrap().original_bytes().as_ref(), [0x00]);
        assert_eq!(db.get_code_by_hash(&code_hash).unwrap().original_bytes().as_ref(), [0x00]);
        assert_eq!(db.get_storage(&address, &key).unwrap(), value);
        assert_eq!(db.get_storage(&address, &key).unwrap(), value);
        assert_eq!(db.get_block_hash(&U256::from(1)).unwrap(), block_hash);
        assert_eq!(db.get_block_hash(&U256::from(1)).unwrap(), block_hash);

        assert_eq!(reads.accounts.get(), 1);
        assert_eq!(reads.code.get(), 1);
        assert_eq!(reads.storage.get(), 1);
        assert_eq!(reads.block_hashes.get(), 1);

        assert!(cached.accounts.contains_key(&address));
        assert!(cached.contracts.contains_key(&code_hash));
        assert_eq!(cached.block_hashes.get(&1), Some(&block_hash));
    }
}
