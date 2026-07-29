//! Speculative EVM database reads.
//!
//! [`SpeculativeDatabase`] delegates account, bytecode, and block-hash reads so execution can
//! follow the contract's real control flow. Storage reads are recorded and return zero instead of
//! reaching the backing database. The recorded read set can then be fetched in parallel before a
//! canonical execution pass.

use alloc::collections::BTreeSet;
use alloy_primitives::{Address, B256, U256};
use core::mem;
use revm::{bytecode::Bytecode, state::AccountInfo, Database};

/// A storage slot requested during speculative execution.
pub type StorageRequest = (Address, U256);

/// A database adapter that records storage reads and returns zero for them.
///
/// Account and bytecode reads still reach the inner database. Returning empty values for those
/// reads would prevent the EVM from entering contract bytecode and discovering its storage slots.
#[derive(Clone, Debug)]
pub struct SpeculativeDatabase<DB> {
    inner: DB,
    speculate_storage: bool,
    storage_requests: BTreeSet<StorageRequest>,
}

impl<DB> SpeculativeDatabase<DB> {
    /// Creates a speculative database backed by `inner`.
    pub const fn new(inner: DB) -> Self {
        Self { inner, speculate_storage: true, storage_requests: BTreeSet::new() }
    }

    /// Creates an adapter that delegates storage reads without recording them.
    ///
    /// This keeps the database type stable when speculative reads are unavailable.
    pub const fn passthrough(inner: DB) -> Self {
        Self { inner, speculate_storage: false, storage_requests: BTreeSet::new() }
    }

    /// Returns the inner database.
    pub const fn inner(&self) -> &DB {
        &self.inner
    }

    /// Returns mutable access to the inner database.
    pub const fn inner_mut(&mut self) -> &mut DB {
        &mut self.inner
    }

    /// Consumes the adapter and returns the inner database.
    pub fn into_inner(self) -> DB {
        self.inner
    }

    /// Returns the storage requests observed since the last call to
    /// [`take_storage_requests`](Self::take_storage_requests).
    pub const fn storage_requests(&self) -> &BTreeSet<StorageRequest> {
        &self.storage_requests
    }

    /// Takes all unique storage requests observed so far.
    pub fn take_storage_requests(&mut self) -> BTreeSet<StorageRequest> {
        mem::take(&mut self.storage_requests)
    }
}

impl<DB: Database> Database for SpeculativeDatabase<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.basic(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        if !self.speculate_storage {
            return self.inner.storage(address, index)
        }
        self.storage_requests.insert((address, index));
        Ok(U256::ZERO)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.inner.block_hash(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;

    #[derive(Clone, Debug, Default)]
    struct TestDatabase {
        storage_reads: usize,
    }

    impl Database for TestDatabase {
        type Error = Infallible;

        fn basic(&mut self, _address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(Some(AccountInfo::default()))
        }

        fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn storage(&mut self, _address: Address, _index: U256) -> Result<U256, Self::Error> {
            self.storage_reads += 1;
            Ok(U256::from(1))
        }

        fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
            Ok(B256::ZERO)
        }
    }

    #[test]
    fn records_unique_storage_reads_without_accessing_inner_database() {
        let address = Address::repeat_byte(0x11);
        let slot = U256::from(42);
        let mut db = SpeculativeDatabase::new(TestDatabase::default());

        assert_eq!(db.storage(address, slot).unwrap(), U256::ZERO);
        assert_eq!(db.storage(address, slot).unwrap(), U256::ZERO);
        assert_eq!(db.inner().storage_reads, 0);
        assert_eq!(db.storage_requests(), &BTreeSet::from([(address, slot)]));

        assert_eq!(db.take_storage_requests(), BTreeSet::from([(address, slot)]));
        assert!(db.storage_requests().is_empty());
    }

    #[test]
    fn delegates_non_storage_reads() {
        let mut db = SpeculativeDatabase::new(TestDatabase::default());

        assert!(db.basic(Address::ZERO).unwrap().is_some());
        assert_eq!(db.code_by_hash(B256::ZERO).unwrap(), Bytecode::default());
        assert_eq!(db.block_hash(1).unwrap(), B256::ZERO);
    }

    #[test]
    fn passthrough_delegates_storage_reads() {
        let mut db = SpeculativeDatabase::passthrough(TestDatabase::default());

        assert_eq!(db.storage(Address::ZERO, U256::ZERO).unwrap(), U256::from(1));
        assert_eq!(db.inner().storage_reads, 1);
        assert!(db.storage_requests().is_empty());
    }
}
