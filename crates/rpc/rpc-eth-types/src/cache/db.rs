//! Helper types to workaround 'higher-ranked lifetime error'
//! <https://github.com/rust-lang/rust/issues/100013> in default implementation of
//! `reth_rpc_eth_api::helpers::Call`.

use alloy_primitives::{Address, B256, U256};
use reth_revm::{database::StateProviderDatabase, DatabaseRef};
use reth_storage_api::StateProvider;
use revm::{
    database::State,
    primitives::HashMap,
    state::{AccountInfo, Bytecode},
    Database, DatabaseCommit,
};

/// Helper alias type for the state's [`State`]
pub type StateCacheDb<'a> = State<StateProviderDatabase<&'a dyn StateProvider>>;

/// Hack to get around 'higher-ranked lifetime error', see
/// <https://github.com/rust-lang/rust/issues/100013>
pub struct StateCacheDbRefMutWrapper<'a, 'b>(pub &'b mut StateCacheDb<'a>);

impl<'a, 'b> core::fmt::Debug for StateCacheDbRefMutWrapper<'a, 'b> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateCacheDbRefMutWrapper").finish_non_exhaustive()
    }
}

impl<'a> Database for StateCacheDbRefMutWrapper<'a, '_> {
    type Error = <StateCacheDb<'a> as Database>::Error;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.0.basic(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.0.code_by_hash(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.0.storage(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.0.block_hash(number)
    }
}

impl<'a> DatabaseRef for StateCacheDbRefMutWrapper<'a, '_> {
    type Error = <StateCacheDb<'a> as Database>::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.0.basic_ref(address)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.0.code_by_hash_ref(code_hash)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.0.storage_ref(address, index)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.0.block_hash_ref(number)
    }
}

impl DatabaseCommit for StateCacheDbRefMutWrapper<'_, '_> {
    fn commit(&mut self, changes: HashMap<Address, revm::state::Account>) {
        self.0.commit(changes)
    }
}
