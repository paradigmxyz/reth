//! State provider database adapter used by EVM execution.

#[cfg(feature = "std")]
use alloy_primitives::{Address, B256};
use core::ops::{Deref, DerefMut};
#[cfg(feature = "std")]
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
    interpreter::Word,
};
#[cfg(feature = "std")]
use reth_storage_api::StateProvider;
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;

/// A database wrapper backed by a [`reth_storage_api::StateProvider`].
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
    DB: StateProvider,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(|account| {
            AccountInfo::new(account.balance, account.nonce, account.get_bytecode_hash(), None)
        }))
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
        let number = number.saturating_to::<u64>();
        Ok(self.0.block_hash(number)?.unwrap_or_default())
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use reth_storage_api::noop::NoopProvider;

    #[test]
    fn missing_block_hash_matches_revm_adapter() {
        let mut native = StateProviderDatabase::new(NoopProvider::mainnet());
        let mut revm = reth_revm::database::StateProviderDatabase::new(NoopProvider::mainnet());
        assert_eq!(
            native.get_block_hash(&Word::from(42)).unwrap(),
            revm::Database::block_hash(&mut revm, 42).unwrap()
        );
    }
}
