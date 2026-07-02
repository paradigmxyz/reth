//! State provider database adapter used by EVM execution.

use alloy_primitives::{Address, BlockNumber, B256, U256};
#[cfg(not(feature = "std"))]
use core::ops::{Deref, DerefMut};
use reth_primitives_traits::Account;
use reth_storage_api::{AccountReader, BlockHashReader, BytecodeReader, StateProvider};
use reth_storage_errors::provider::ProviderResult;

#[cfg(feature = "std")]
pub use reth_storage_api::EvmStateProviderDatabase as StateProviderDatabase;

/// A helper trait responsible for providing state necessary for EVM execution.
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

/// A database wrapper backed by an [`EvmStateProvider`].
#[cfg(not(feature = "std"))]
#[derive(Clone)]
pub struct StateProviderDatabase<DB>(pub DB);

#[cfg(not(feature = "std"))]
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

#[cfg(not(feature = "std"))]
impl<DB> core::fmt::Debug for StateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateProviderDatabase").finish_non_exhaustive()
    }
}

#[cfg(not(feature = "std"))]
impl<DB> AsRef<DB> for StateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

#[cfg(not(feature = "std"))]
impl<DB> Deref for StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(not(feature = "std"))]
impl<DB> DerefMut for StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
