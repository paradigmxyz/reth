//! Database read cache helpers for payload building.

use alloy_primitives::{
    map::{AddressMap, B256Map, Entry, HashMap, U256Map},
    Address, B256, U256,
};
use core::cell::RefCell;
use reth_execution_types::{AccountInfo, Bytecode, Database, DatabaseRef};

/// A container type that caches reads from an underlying [`DatabaseRef`].
#[derive(Debug, Clone, Default)]
pub struct CachedReads {
    /// Block state account with storage.
    pub accounts: AddressMap<CachedAccount>,
    /// Created contracts.
    pub contracts: B256Map<Bytecode>,
    /// Block hash mapped to the block number.
    pub block_hashes: HashMap<u64, B256>,
}

impl CachedReads {
    /// Gets a [`DatabaseRef`] that will cache reads from the given database.
    pub const fn as_db<DB>(&mut self, db: DB) -> CachedReadsDBRef<'_, DB> {
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
    pub fn extend(&mut self, other: Self) {
        self.accounts.extend(other.accounts);
        self.contracts.extend(other.contracts);
        self.block_hashes.extend(other.block_hashes);
    }
}

/// A [`Database`] that caches reads inside [`CachedReads`].
#[derive(Debug)]
pub struct CachedReadsDbMut<'a, DB> {
    /// The cache of reads.
    pub cached: &'a mut CachedReads,
    /// The underlying database.
    pub db: DB,
}

impl<'a, DB> CachedReadsDbMut<'a, DB> {
    /// Converts this [`Database`] implementation into a [`DatabaseRef`] that will still cache
    /// reads.
    pub const fn into_db(self) -> CachedReadsDBRef<'a, DB> {
        CachedReadsDBRef { inner: RefCell::new(self) }
    }

    /// Returns access to wrapped [`DatabaseRef`].
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

impl<DB: DatabaseRef> Database for CachedReadsDbMut<'_, DB> {
    type Error = <DB as DatabaseRef>::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let basic = match self.cached.accounts.entry(address) {
            Entry::Occupied(entry) => entry.get().info.clone(),
            Entry::Vacant(entry) => {
                entry.insert(CachedAccount::new(self.db.basic_ref(address)?)).info.clone()
            }
        };
        Ok(basic)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let code = match self.cached.contracts.entry(code_hash) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => entry.insert(self.db.code_by_hash_ref(code_hash)?).clone(),
        };
        Ok(code)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        match self.cached.accounts.entry(address) {
            Entry::Occupied(mut acc_entry) => match acc_entry.get_mut().storage.entry(index) {
                Entry::Occupied(entry) => Ok(*entry.get()),
                Entry::Vacant(entry) => Ok(*entry.insert(self.db.storage_ref(address, index)?)),
            },
            Entry::Vacant(acc_entry) => {
                let info = self.db.basic_ref(address)?;
                let (account, value) = if info.is_some() {
                    let value = self.db.storage_ref(address, index)?;
                    let mut account = CachedAccount::new(info);
                    account.storage.insert(index, value);
                    (account, value)
                } else {
                    (CachedAccount::new(info), U256::ZERO)
                };
                acc_entry.insert(account);
                Ok(value)
            }
        }
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        let hash = match self.cached.block_hashes.entry(number) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => *entry.insert(self.db.block_hash_ref(number)?),
        };
        Ok(hash)
    }
}

/// A [`DatabaseRef`] that caches reads inside [`CachedReads`].
#[derive(Debug)]
pub struct CachedReadsDBRef<'a, DB> {
    /// The inner cache reads db mut.
    pub inner: RefCell<CachedReadsDbMut<'a, DB>>,
}

impl<DB: DatabaseRef> DatabaseRef for CachedReadsDBRef<'_, DB> {
    type Error = <DB as DatabaseRef>::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.borrow_mut().basic(address)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.borrow_mut().code_by_hash(code_hash)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.inner.borrow_mut().storage(address, index)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.inner.borrow_mut().block_hash(number)
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
    fn new(info: Option<AccountInfo>) -> Self {
        Self { info, storage: U256Map::default() }
    }
}
