//! Evm2 database adapter for state providers.

use crate::{AccountReader, BlockHashReader, BytecodeReader, StateProvider};
use alloy_primitives::{Address, BlockNumber, B256, U256};
use core::{
    mem,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
    interpreter::Word,
};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderError;

/// An evm2 [`Database`] implementation backed by a Reth [`StateProvider`].
#[derive(Clone)]
pub struct Evm2StateProviderDatabase<DB>(pub DB);

impl<DB> Evm2StateProviderDatabase<DB> {
    /// Creates a new evm2 database adapter.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consumes the adapter and returns the wrapped state provider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB> core::fmt::Debug for Evm2StateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Evm2StateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for Evm2StateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

impl<DB> Deref for Evm2StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB> DerefMut for Evm2StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB> Database for Evm2StateProviderDatabase<DB>
where
    DB: StateProvider + Send + 'static,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(<DB as AccountReader>::basic_account(&self.0, address)?.map(account_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(<DB as BytecodeReader>::bytecode_by_hash(&self.0, code_hash)?
            .map(|bytecode| Bytecode::new_raw(bytecode.original_bytes()))
            .unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        <DB as BlockHashReader>::block_hash(&self.0, number)
    }
}

/// An evm2 [`Database`] implementation backed by a borrowed Reth [`StateProvider`].
///
/// evm2 database objects must be `'static` for downcasting. This adapter is only valid while the
/// borrowed provider passed to [`Self::new`] is alive and must not escape the synchronous execution
/// call that created it.
#[derive(Clone, Copy)]
pub struct BorrowedEvm2StateProviderDatabase {
    provider: NonNull<dyn StateProvider>,
}

impl BorrowedEvm2StateProviderDatabase {
    /// Creates a new borrowed evm2 database adapter.
    ///
    /// # Safety
    ///
    /// The returned adapter erases the lifetime of `provider` to satisfy evm2's [`Database`]
    /// downcasting requirements. It must not be used after `provider` is dropped and must not
    /// escape the synchronous execution call that created it.
    pub unsafe fn new(provider: &dyn StateProvider) -> Self {
        let provider = NonNull::from(provider);
        // SAFETY: The caller guarantees the erased lifetime remains valid for every use of the
        // returned adapter.
        let provider = unsafe {
            mem::transmute::<NonNull<dyn StateProvider + '_>, NonNull<dyn StateProvider + 'static>>(
                provider,
            )
        };
        Self { provider }
    }

    fn provider(&self) -> &dyn StateProvider {
        // SAFETY: `provider` is created from a valid shared reference in `new`. Callers must keep
        // that provider alive for the duration of synchronous evm2 execution.
        unsafe { self.provider.as_ref() }
    }
}

impl core::fmt::Debug for BorrowedEvm2StateProviderDatabase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BorrowedEvm2StateProviderDatabase").finish_non_exhaustive()
    }
}

// SAFETY: The adapter only exposes shared `StateProvider` reads and is used for synchronous evm2
// execution. Sending it is sound under the same assumptions as sending the underlying borrowed
// provider reference for read-only access.
unsafe impl Send for BorrowedEvm2StateProviderDatabase {}

impl Database for BorrowedEvm2StateProviderDatabase {
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(AccountReader::basic_account(self.provider(), address)?.map(account_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(BytecodeReader::bytecode_by_hash(self.provider(), code_hash)?
            .map(|bytecode| Bytecode::new_raw(bytecode.original_bytes()))
            .unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.provider().storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        BlockHashReader::block_hash(self.provider(), number)
    }
}

fn account_to_evm2(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

fn u256_to_u64_saturating(value: U256) -> BlockNumber {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}
