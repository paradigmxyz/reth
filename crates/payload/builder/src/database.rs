//! Database adapters for payload building.

use hashbrown::{hash_map::Entry, HashMap};
use reth_primitives::U256;
use revm_primitives::{
    db::{Database, DatabaseRef},
    AccountInfo, Address, Bytecode, B256,
};
use std::cell::RefCell;

/// A container type that caches all [DatabaseRef] reads from an underlying [DatabaseRef].
///
/// This is intended to be used in conjunction with [CacheDB](reth_revm_primitives::db::CacheDB)
/// during payload building which repeatedly accesses the same data.
///
/// # Example
///
/// ```
/// use revm_primitives::db::DatabaseRef;
/// use reth_payload_builder::database::CachedReads;
/// use reth_revm_primitives::db::CacheDB;
///
/// fn build_payload<DB: DatabaseRef>(db: DB) {
///   let mut cached_reads = CachedReads::default();
///   let db_ref = cached_reads.as_db(db);
///
///   // this is `Database` and can be used to build a payload, it never writes to `CachedReads` or the underlying database, but all reads from the underlying database are cached in `CachedReads`.
///   // Subsequent payload build attempts can use cached reads and avoid hitting the underlying database.
///   let db = CacheDB::new(db_ref);
///
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct CachedReads {
    accounts: HashMap<Address, CachedAccount>,
    contracts: HashMap<B256, Bytecode>,
    block_hashes: HashMap<U256, B256>,
}

// === impl CachedReads ===

impl CachedReads {
    /// Gets a [DatabaseRef] that will cache reads from the given database.
    pub fn as_db<DB>(&mut self, db: DB) -> CachedReadsDBRef<'_, DB> {
        CachedReadsDBRef { inner: RefCell::new(self.as_db_mut(db)) }
    }

    fn as_db_mut<DB>(&mut self, db: DB) -> CachedReadsDbMut<'_, DB> {
        CachedReadsDbMut { cached: self, db }
    }
}

#[derive(Debug)]
struct CachedReadsDbMut<'a, DB> {
    cached: &'a mut CachedReads,
    db: DB,
}

impl<'a, DB: DatabaseRef> Database for CachedReadsDbMut<'a, DB> {
    type Error = <DB as DatabaseRef>::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let basic = match self.cached.accounts.entry(address) {
            Entry::Occupied(entry) => entry.get().info.clone(),
            Entry::Vacant(entry) => {
                entry.insert(CachedAccount::new(self.db.basic(address)?)).info.clone()
            }
        };
        Ok(basic)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let code = match self.cached.contracts.entry(code_hash) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => entry.insert(self.db.code_by_hash(code_hash)?).clone(),
        };
        Ok(code)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        match self.cached.accounts.entry(address) {
            Entry::Occupied(mut acc_entry) => {
                let acc_entry = acc_entry.get_mut();
                match acc_entry.storage.entry(index) {
                    Entry::Occupied(entry) => Ok(*entry.get()),
                    Entry::Vacant(entry) => {
                        let slot = self.db.storage(address, index)?;
                        entry.insert(slot);
                        Ok(slot)
                    }
                }
            }
            Entry::Vacant(acc_entry) => {
                // acc needs to be loaded for us to access slots.
                let info = self.db.basic(address)?;
                let (account, value) = if info.is_some() {
                    let value = self.db.storage(address, index)?;
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

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        let code = match self.cached.block_hashes.entry(number) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => *entry.insert(self.db.block_hash(number)?),
        };
        Ok(code)
    }
}

/// A [DatabaseRef] that caches reads inside [CachedReads].
///
/// This is intended to be used as the [DatabaseRef] for
/// [CacheDB](reth_revm_primitives::db::CacheDB) for repeated payload build jobs.
#[derive(Debug)]
pub struct CachedReadsDBRef<'a, DB> {
    inner: RefCell<CachedReadsDbMut<'a, DB>>,
}

impl<'a, DB: DatabaseRef> DatabaseRef for CachedReadsDBRef<'a, DB> {
    type Error = <DB as DatabaseRef>::Error;

    fn basic(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.borrow_mut().basic(address)
    }

    fn code_by_hash(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.borrow_mut().code_by_hash(code_hash)
    }

    fn storage(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.inner.borrow_mut().storage(address, index)
    }

    fn block_hash(&self, number: U256) -> Result<B256, Self::Error> {
        self.inner.borrow_mut().block_hash(number)
    }
}

#[derive(Debug, Clone)]
struct CachedAccount {
    info: Option<AccountInfo>,
    storage: HashMap<U256, U256>,
}

impl CachedAccount {
    fn new(info: Option<AccountInfo>) -> Self {
        Self { info, storage: HashMap::new() }
    }
}
