use reth_interfaces::RethError;
use reth_primitives::{Address, B256, KECCAK_EMPTY, U256};
use reth_provider::{ProviderError, StateProvider};
use revm::{
    db::{CacheDB, DatabaseRef},
    primitives::{AccountInfo, Bytecode},
    Database, StateDBBox,
};

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

    /// Return inner state reference
    pub fn state(&self) -> &DB {
        &self.0
    }

    /// Return inner state mutable reference
    pub fn state_mut(&mut self) -> &mut DB {
        &mut self.0
    }

    /// Consume State and return inner StateProvider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB: StateProvider> Database for StateProviderDatabase<DB> {
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(|account| AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let bytecode = self.0.bytecode_by_hash(code_hash)?;

        Ok(bytecode.map(|b| b.0).unwrap_or_else(Bytecode::new))
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let index = B256::new(index.to_be_bytes());
        let ret = self.0.storage(address, index)?.unwrap_or_default();
        Ok(ret)
    }

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        // The `number` represents the block number, so it is safe to cast it to u64.
        Ok(self.0.block_hash(number.try_into().unwrap())?.unwrap_or_default())
    }
}

impl<DB: StateProvider> DatabaseRef for StateProviderDatabase<DB> {
    type Error = <Self as Database>::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(|account| AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let bytecode = self.0.bytecode_by_hash(code_hash)?;

        if let Some(bytecode) = bytecode {
            Ok(bytecode.0)
        } else {
            Ok(Bytecode::new())
        }
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let index = B256::new(index.to_be_bytes());
        let ret = self.0.storage(address, index)?.unwrap_or_default();
        Ok(ret)
    }

    fn block_hash_ref(&self, number: U256) -> Result<B256, Self::Error> {
        // Note: this unwrap is potentially unsafe
        Ok(self.0.block_hash(number.try_into().unwrap())?.unwrap_or_default())
    }
}
