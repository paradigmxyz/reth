//! State provider database adapter used by EVM execution.

#[cfg(feature = "std")]
use alloy_eips::BlockHashOrNumber;
#[cfg(feature = "std")]
use alloy_primitives::{Address, BlockNumber, B256, U256};
use core::ops::{Deref, DerefMut};
#[cfg(feature = "std")]
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
    interpreter::Word,
};
#[cfg(feature = "std")]
use reth_primitives_traits::Account;
#[cfg(feature = "std")]
use reth_storage_api::EvmStateProvider;
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;

/// A database wrapper backed by an [`EvmStateProvider`](reth_storage_api::EvmStateProvider).
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
        Ok(self
            .0
            .bytecode_by_hash(code_hash)?
            .map(|code| reth_execution_types::native_bytecode(&code.0))
            .unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<B256, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        self.0
            .block_hash(number)?
            .ok_or(ProviderError::HeaderNotFound(BlockHashOrNumber::Number(number)))
    }
}

#[cfg(feature = "std")]
fn account_to_evm(account: Account) -> AccountInfo {
    reth_execution_types::native_account(&account.into())
}

#[cfg(feature = "std")]
fn u256_to_u64_saturating(value: U256) -> BlockNumber {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}
