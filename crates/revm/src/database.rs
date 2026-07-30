use crate::primitives::alloy_primitives::{BlockNumber, StorageKey, StorageValue};
use alloy_primitives::{Address, B256, U256};
use core::ops::{Deref, DerefMut};
use reth_primitives_traits::Account;
use reth_storage_api::{AccountReader, BlockHashReader, BytecodeReader, StateProvider};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use revm::{bytecode::Bytecode, state::AccountInfo, Database, DatabaseRef};

/// A helper trait responsible for providing state necessary for EVM execution.
///
/// This serves as the data layer for [`Database`].
pub trait EvmStateProvider {
    /// Get basic account information.
    ///
    /// Returns [`None`] if the account doesn't exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>>;

    /// Get the hash of the block with the given number. Returns [`None`] if no block with this
    /// number exists.
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>>;

    /// Get account code by hash.
    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> ProviderResult<Option<reth_primitives_traits::Bytecode>>;

    /// Get storage of the given account.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>>;
}

// Blanket implementation of EvmStateProvider for any type that implements StateProvider.
impl<T: StateProvider> EvmStateProvider for T {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        <T as AccountReader>::basic_account(self, address)
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        <T as BlockHashReader>::block_hash(self, number)
    }

    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        <T as BytecodeReader>::bytecode_by_hash(self, code_hash)
    }

    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        <T as StateProvider>::storage(self, account, storage_key)
    }
}

/// A [Database] and [`DatabaseRef`] implementation that uses [`EvmStateProvider`] as the underlying
/// data source.
#[derive(Clone)]
pub struct StateProviderDatabase<DB>(pub DB);

impl<DB> StateProviderDatabase<DB> {
    /// Create new State with generic `StateProvider`.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consume State and return inner `StateProvider`.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB> core::fmt::Debug for StateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for StateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

impl<DB> Deref for StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB> DerefMut for StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB: EvmStateProvider> Database for StateProviderDatabase<DB> {
    type Error = ProviderError;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    /// Retrieves the storage value at a specific index for a given address.
    ///
    /// Returns `Ok` with the storage value, or the default value if not found.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    /// Retrieves the block hash for a given block number.
    ///
    /// Returns `Ok` with the block hash if found, or the default hash otherwise.
    /// Note: It safely casts the `number` to `u64`.
    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}

impl<DB: EvmStateProvider> DatabaseRef for StateProviderDatabase<DB> {
    type Error = <Self as Database>::Error;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.basic_account(&address)?.map(Into::into))
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.bytecode_by_hash(&code_hash)?.unwrap_or_default().0)
    }

    /// Retrieves the storage value at a specific index for a given address.
    ///
    /// Returns `Ok` with the storage value, or the default value if not found.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self.0.storage(address, B256::new(index.to_be_bytes()))?.unwrap_or_default())
    }

    /// Retrieves the block hash for a given block number.
    ///
    /// Returns `Ok` with the block hash if found, or the default hash otherwise.
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        // Get the block hash or default hash with an attempt to convert U256 block number to u64
        Ok(self.0.block_hash(number)?.unwrap_or_default())
    }
}

/// A [`DatabaseRef`] backed account info reader.
///
/// This adapts account and bytecode reads from revm's database interface back into Reth's storage
/// reader traits. It is intentionally not a full [`StateProvider`] because [`DatabaseRef`] does
/// not expose roots or proofs.
///
/// Note: [`DatabaseRef::code_by_hash_ref`] returns [`Bytecode`] directly, so this adapter cannot
/// distinguish missing bytecode from the database's default bytecode and wraps whatever the
/// database returns in `Some`.
#[derive(Clone)]
pub struct DatabaseStateProvider<DB>(pub DB);

impl<DB> DatabaseStateProvider<DB> {
    /// Create a new database-backed state reader.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consume self and return the inner database.
    pub fn into_inner(self) -> DB {
        self.0
    }

    /// Returns the inner database.
    pub const fn inner(&self) -> &DB {
        &self.0
    }
}

impl<DB> core::fmt::Debug for DatabaseStateProvider<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DatabaseStateProvider").finish_non_exhaustive()
    }
}

impl<DB> AccountReader for DatabaseStateProvider<DB>
where
    DB: DatabaseRef<Error = ProviderError>,
{
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        Ok(self.0.basic_ref(*address)?.map(Into::into))
    }
}

impl<DB> BytecodeReader for DatabaseStateProvider<DB>
where
    DB: DatabaseRef<Error = ProviderError>,
{
    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        Ok(Some(reth_primitives_traits::Bytecode(self.0.code_by_hash_ref(*code_hash)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cached::CachedReads;
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_primitives::Bytes;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone)]
    struct CountingDatabaseRef {
        address: Address,
        code_hash: B256,
        account: Option<AccountInfo>,
        bytecode: Bytecode,
        account_reads: Arc<AtomicUsize>,
        bytecode_reads: Arc<AtomicUsize>,
        fail_account_reads: bool,
        fail_bytecode_reads: bool,
    }

    impl CountingDatabaseRef {
        fn new(address: Address, account: Option<AccountInfo>, bytecode: Bytecode) -> Self {
            let code_hash = account.as_ref().map(|account| account.code_hash).unwrap_or_default();
            Self {
                address,
                code_hash,
                account,
                bytecode,
                account_reads: Arc::default(),
                bytecode_reads: Arc::default(),
                fail_account_reads: false,
                fail_bytecode_reads: false,
            }
        }
    }

    impl DatabaseRef for CountingDatabaseRef {
        type Error = ProviderError;

        fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            if self.fail_account_reads {
                return Err(ProviderError::UnsupportedProvider)
            }

            self.account_reads.fetch_add(1, Ordering::Relaxed);
            Ok((address == self.address).then(|| self.account.clone()).flatten())
        }

        fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
            if self.fail_bytecode_reads {
                return Err(ProviderError::UnsupportedProvider)
            }

            self.bytecode_reads.fetch_add(1, Ordering::Relaxed);
            Ok(if code_hash == self.code_hash {
                self.bytecode.clone()
            } else {
                Bytecode::default()
            })
        }

        fn storage_ref(&self, _address: Address, _index: U256) -> Result<U256, Self::Error> {
            Ok(U256::ZERO)
        }

        fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
            Ok(B256::ZERO)
        }
    }

    #[test]
    fn database_state_provider_maps_missing_account() {
        let address = Address::repeat_byte(0x01);
        let db = CountingDatabaseRef::new(address, None, Bytecode::default());
        let provider = DatabaseStateProvider::new(db);

        assert_eq!(provider.basic_account(&address).unwrap(), None);
    }

    #[test]
    fn database_state_provider_maps_empty_code_hash() {
        let address = Address::repeat_byte(0x01);
        let account = AccountInfo {
            nonce: 7,
            balance: U256::from(42),
            code_hash: KECCAK_EMPTY,
            code: None,
            account_id: None,
        };
        let db = CountingDatabaseRef::new(address, Some(account), Bytecode::default());
        let provider = DatabaseStateProvider::new(db);

        assert_eq!(
            provider.basic_account(&address).unwrap(),
            Some(Account { nonce: 7, balance: U256::from(42), bytecode_hash: None })
        );
    }

    #[test]
    fn database_state_provider_maps_code_hash_and_bytecode() {
        let address = Address::repeat_byte(0x01);
        let code_hash = B256::repeat_byte(0x42);
        let bytecode = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let account = AccountInfo {
            nonce: 7,
            balance: U256::from(42),
            code_hash,
            code: Some(bytecode.clone()),
            account_id: None,
        };
        let db = CountingDatabaseRef::new(address, Some(account), bytecode.clone());
        let provider = DatabaseStateProvider::new(db);

        assert_eq!(
            provider.basic_account(&address).unwrap(),
            Some(Account { nonce: 7, balance: U256::from(42), bytecode_hash: Some(code_hash) })
        );
        assert_eq!(
            provider.bytecode_by_hash(&code_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(bytecode))
        );
    }

    #[test]
    fn database_state_provider_wraps_default_bytecode_for_unknown_hash() {
        let address = Address::repeat_byte(0x01);
        let unknown_hash = B256::repeat_byte(0x42);
        let db = CountingDatabaseRef::new(address, None, Bytecode::default());
        let provider = DatabaseStateProvider::new(db);

        assert_eq!(
            provider.bytecode_by_hash(&unknown_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(Bytecode::default()))
        );
    }

    #[test]
    fn database_state_provider_propagates_database_errors() {
        let address = Address::repeat_byte(0x01);
        let code_hash = B256::repeat_byte(0x42);
        let db = CountingDatabaseRef {
            fail_account_reads: true,
            fail_bytecode_reads: true,
            ..CountingDatabaseRef::new(address, None, Bytecode::default())
        };
        let provider = DatabaseStateProvider::new(db);

        assert!(matches!(
            provider.basic_account(&address),
            Err(ProviderError::UnsupportedProvider)
        ));
        assert!(matches!(
            provider.bytecode_by_hash(&code_hash),
            Err(ProviderError::UnsupportedProvider)
        ));
    }

    #[test]
    fn database_state_provider_uses_cached_reads() {
        let address = Address::repeat_byte(0x01);
        let code_hash = B256::repeat_byte(0x42);
        let bytecode = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let account = AccountInfo {
            nonce: 7,
            balance: U256::from(42),
            code_hash,
            code: Some(bytecode.clone()),
            account_id: None,
        };
        let db = CountingDatabaseRef::new(address, Some(account), bytecode.clone());
        let account_reads = db.account_reads.clone();
        let bytecode_reads = db.bytecode_reads.clone();
        let mut cached_reads = CachedReads::default();
        let provider = DatabaseStateProvider::new(cached_reads.as_db(db));

        assert_eq!(
            provider.basic_account(&address).unwrap(),
            Some(Account { nonce: 7, balance: U256::from(42), bytecode_hash: Some(code_hash) })
        );
        assert_eq!(
            provider.basic_account(&address).unwrap(),
            Some(Account { nonce: 7, balance: U256::from(42), bytecode_hash: Some(code_hash) })
        );
        assert_eq!(account_reads.load(Ordering::Relaxed), 1);

        assert_eq!(
            provider.bytecode_by_hash(&code_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(bytecode.clone()))
        );
        assert_eq!(
            provider.bytecode_by_hash(&code_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(bytecode))
        );
        assert_eq!(bytecode_reads.load(Ordering::Relaxed), 1);
    }
}
