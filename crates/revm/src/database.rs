use reth_interfaces::RethError;
use reth_primitives::{Address, B256, KECCAK_EMPTY, U256};
use reth_provider::{ProviderError, StateProvider};
use revm::{
    db::{CacheDB, DatabaseRef},
    primitives::{AccountInfo, Bytecode},
    Database, StateDBBox,
};
use std::ops::{Deref, DerefMut};

/// SubState of database. Uses revm internal cache with binding to reth StateProvider trait.
pub type SubState<DB> = CacheDB<StateProviderDatabase<DB>>;

/// State boxed database with reth Error.
pub type RethStateDBBox<'a> = StateDBBox<'a, RethError>;

/// Wrapper around StateProvider that implements revm database trait
#[derive(Debug, Clone)]
pub struct StateProviderDatabase<DB: StateProvider>(pub DB);

impl<DB: StateProvider> StateProviderDatabase<DB> {
    /// Create new State with generic StateProvider.
    pub fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consume State and return inner StateProvider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB: StateProvider> Deref for StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB: StateProvider> DerefMut for StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB: StateProvider> Database for StateProviderDatabase<DB> {
    type Error = ProviderError;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        DatabaseRef::basic_ref(self, address)
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        DatabaseRef::code_by_hash_ref(self, code_hash)
    }

    /// Retrieves the storage value at a specific index for a given address.
    ///
    /// Returns `Ok` with the storage value, or the default value if not found.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        DatabaseRef::storage_ref(self, address, index)
    }

    /// Retrieves the block hash for a given block number.
    ///
    /// Returns `Ok` with the block hash if found, or the default hash otherwise.
    /// Note: It safely casts the `number` to `u64`.
    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        DatabaseRef::block_hash_ref(self, number)
    }
}

impl<DB: StateProvider> DatabaseRef for StateProviderDatabase<DB> {
    type Error = <Self as Database>::Error;

    /// Retrieves basic account information for a given address.
    ///
    /// Returns `Ok` with `Some(AccountInfo)` if the account exists,
    /// `None` if it doesn't, or an error if encountered.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.basic_account(address)?.map(|account| AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }))
    }

    /// Retrieves the bytecode associated with a given code hash.
    ///
    /// Returns `Ok` with the bytecode if found, or the default bytecode otherwise.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.bytecode_by_hash(code_hash)?.unwrap_or_default().0)
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
    fn block_hash_ref(&self, number: U256) -> Result<B256, Self::Error> {
        // Attempt to convert U256 to u64
        let block_number = match number.try_into() {
            Ok(value) => value,
            Err(_) => return Err(Self::Error::BlockNumberOverflow(number)),
        };

        // Get the block hash or default hash
        Ok(self.0.block_hash(block_number)?.unwrap_or_default())
    }
}
