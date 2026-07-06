//! State provider database adapter used by EVM execution.

use alloy_primitives::{Address, BlockNumber, B256, U256};
use core::ops::{Deref, DerefMut};
#[cfg(feature = "std")]
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database, DbResult, DynDatabase},
    interpreter::Word,
    ErrorCode,
};
use reth_primitives_traits::Account;
use reth_storage_api::{AccountReader, BlockHashReader, BytecodeReader, StateProvider};
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;
use reth_storage_errors::provider::ProviderResult;

/// A helper trait responsible for providing state necessary for EVM execution.
pub(crate) trait EvmStateProvider {
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
    fn storage(&self, account: Address, storage_key: B256) -> ProviderResult<Option<U256>>;
}

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

    fn storage(&self, account: Address, storage_key: B256) -> ProviderResult<Option<U256>> {
        <T as StateProvider>::storage(self, account, storage_key)
    }
}

/// A database wrapper backed by a [`StateProvider`].
#[derive(Clone)]
pub struct StateProviderDatabase<DB>(pub DB);

impl<DB> StateProviderDatabase<DB> {
    /// Creates a new database wrapper with the given state provider.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consumes the wrapper and returns the inner state provider.
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
        &self.0
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

/// Borrowed adapter for reusing a dynamic database without moving it into an EVM.
pub struct BorrowedDatabase<'a, DB: ?Sized>(&'a mut DB);

impl<'a, DB: ?Sized> BorrowedDatabase<'a, DB> {
    /// Creates a new borrowed database adapter.
    pub const fn new(db: &'a mut DB) -> Self {
        Self(db)
    }
}

impl<DB> core::fmt::Debug for BorrowedDatabase<'_, DB>
where
    DB: ?Sized,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BorrowedDatabase").finish_non_exhaustive()
    }
}

#[cfg(feature = "std")]
impl<DB> DynDatabase for BorrowedDatabase<'_, DB>
where
    DB: DynDatabase + ?Sized,
{
    fn get_account(&mut self, address: &Address) -> DbResult<Option<AccountInfo>> {
        self.0.get_account(address)
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> DbResult<Bytecode> {
        self.0.get_code_by_hash(code_hash)
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> DbResult<Word> {
        self.0.get_storage(address, key)
    }

    fn get_block_hash(&mut self, number: &Word) -> DbResult<Option<B256>> {
        self.0.get_block_hash(number)
    }

    fn error(&mut self, code: ErrorCode) -> evm2::AnyError {
        self.0.error(code)
    }
}

#[cfg(feature = "std")]
impl<DB> Database for StateProviderDatabase<DB>
where
    DB: EvmStateProvider,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(account_to_evm))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(self.0.bytecode_by_hash(code_hash)?.map(Into::into).unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        self.0.block_hash(u256_to_u64_saturating(*number))
    }
}

#[cfg(feature = "std")]
fn account_to_evm(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

#[cfg(feature = "std")]
fn u256_to_u64_saturating(value: U256) -> BlockNumber {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}
