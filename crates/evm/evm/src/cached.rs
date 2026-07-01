//! Database adapters for payload building.

use alloc::rc::Rc;
use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap, U256Map},
    Address, B256, U256,
};
use core::cell::RefCell;
pub use evm2::{bytecode::Bytecode, evm::AccountInfo};
use evm2::{evm::Database, interpreter::Word};

/// A container type that caches reads for payload pre-cache bookkeeping.
#[derive(Debug, Clone, Default)]
pub struct CachedReads {
    /// Block state account with storage.
    pub accounts: AddressMap<CachedAccount>,
    /// Created contracts.
    pub contracts: B256Map<Bytecode>,
    /// Block hash mapped to the block number.
    pub block_hashes: HashMap<u64, Option<B256>>,
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

    /// Gets a database wrapper that will cache reads from the given database.
    pub fn as_db<DB>(&mut self, db: DB) -> CachedReadsDb<DB> {
        self.as_db_mut(db).into_db()
    }

    /// Gets a mutable database wrapper that will cache reads from the underlying database.
    pub fn as_db_mut<DB>(&mut self, db: DB) -> CachedReadsDbMut<DB> {
        CachedReadsDbMut { cached: Rc::new(RefCell::new(self.clone())), db }
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

/// A database wrapper that caches reads inside [`CachedReads`].
pub type CachedReadsDb<DB> = CachedReadsDbMut<DB>;

/// Mutable cache wrapper for cache-aware call sites.
#[derive(Debug, Clone)]
pub struct CachedReadsDbMut<DB> {
    /// The cache of reads.
    pub cached: Rc<RefCell<CachedReads>>,
    /// The underlying database.
    pub db: DB,
}

impl<DB> CachedReadsDbMut<DB> {
    /// Converts this mutable wrapper into a database wrapper.
    pub const fn into_db(self) -> Self {
        self
    }

    /// Returns access to wrapped database.
    pub const fn inner(&self) -> &DB {
        &self.db
    }

    /// Returns the currently cached reads.
    pub fn cached_reads(&self) -> CachedReads {
        self.cached.borrow().clone()
    }

    /// Copies the currently cached reads into the given cache.
    pub fn sync(&self, cached_reads: &mut CachedReads) {
        *cached_reads = self.cached_reads();
    }
}

impl<DB, T> AsRef<T> for CachedReadsDbMut<DB>
where
    DB: AsRef<T>,
{
    fn as_ref(&self) -> &T {
        self.inner().as_ref()
    }
}

impl<DB> Database for CachedReadsDbMut<DB>
where
    DB: Database,
{
    type Error = DB::Error;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        if let Some(account) = self.cached.borrow().accounts.get(address) {
            return Ok(account.info.clone())
        }

        let info = self.db.get_account(address)?;
        self.cached.borrow_mut().accounts.insert(*address, CachedAccount::new(info.clone()));
        Ok(info)
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        if let Some(code) = self.cached.borrow().contracts.get(code_hash) {
            return Ok(code.clone())
        }

        let code = self.db.get_code_by_hash(code_hash)?;
        self.cached.borrow_mut().contracts.insert(*code_hash, code.clone());
        Ok(code)
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        {
            let cached = self.cached.borrow();
            if let Some(account) = cached.accounts.get(address) {
                if let Some(value) = account.storage.get(key) {
                    return Ok(*value)
                }

                if account.info.is_none() {
                    return Ok(U256::ZERO)
                }
            }
        }

        if !self.cached.borrow().accounts.contains_key(address) {
            let info = self.db.get_account(address)?;
            if info.is_none() {
                self.cached.borrow_mut().accounts.insert(*address, CachedAccount::new(None));
                return Ok(U256::ZERO)
            }

            self.cached.borrow_mut().accounts.insert(*address, CachedAccount::new(info));
        }

        let value = self.db.get_storage(address, key)?;
        self.cached
            .borrow_mut()
            .accounts
            .entry(*address)
            .or_insert_with(|| CachedAccount::new(None))
            .storage
            .insert(*key, value);
        Ok(value)
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        if let Some(hash) = self.cached.borrow().block_hashes.get(&number) {
            return Ok(*hash)
        }

        let hash = self.db.get_block_hash(&U256::from(number))?;
        self.cached.borrow_mut().block_hashes.insert(number, hash);
        Ok(hash)
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

fn u256_to_u64_saturating(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use evm2::interpreter::Word;
    use std::{error::Error, fmt, rc::Rc};

    #[derive(Clone, Debug)]
    struct CountingDb {
        account: Option<AccountInfo>,
        code: Bytecode,
        storage: Word,
        block_hash: Option<B256>,
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

        fn get_block_hash(&mut self, _number: &Word) -> Result<Option<B256>, Self::Error> {
            self.reads.block_hashes.set(self.reads.block_hashes.get() + 1);
            Ok(self.block_hash)
        }
    }

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
            cache.block_hashes.insert(1, Some(hash1));
            cache
        };

        let additional = {
            let mut cache = CachedReads::default();
            cache.accounts.insert(address2, CachedAccount::new(Some(AccountInfo::default())));
            cache.contracts.insert(hash2, Bytecode::default());
            cache.block_hashes.insert(2, Some(hash2));
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
        assert_eq!(primary.block_hashes.get(&1), Some(&Some(hash1)));
        assert_eq!(primary.block_hashes.get(&2), Some(&Some(hash2)));
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
            account: Some(AccountInfo {
                balance: U256::from(1),
                nonce: 2,
                code_hash,
                code: None,
                _non_exhaustive: (),
            }),
            code: Bytecode::new_raw([0x00].as_slice().into()),
            storage: value,
            block_hash: Some(block_hash),
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
        assert_eq!(db.get_block_hash(&U256::from(1)).unwrap(), Some(block_hash));
        assert_eq!(db.get_block_hash(&U256::from(1)).unwrap(), Some(block_hash));

        assert_eq!(reads.accounts.get(), 1);
        assert_eq!(reads.code.get(), 1);
        assert_eq!(reads.storage.get(), 1);
        assert_eq!(reads.block_hashes.get(), 1);

        db.sync(&mut cached);
        assert!(cached.accounts.contains_key(&address));
        assert!(cached.contracts.contains_key(&code_hash));
        assert_eq!(cached.block_hashes.get(&1), Some(&Some(block_hash)));
    }
}
