//! A database over a block's parent state that worker threads share.

use alloy_primitives::{Address, B256};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database as _, DatabaseRef},
    interpreter::Word,
};
use reth_errors::ProviderResult;
use reth_evm::database::StateProviderDatabase;
use reth_provider::{EvmStateProviderBox, ProviderError};
use std::cell::RefCell;
use thread_local::ThreadLocal;

/// Reads a block's parent state through one state provider per thread, so that the threads of a
/// parallel block executor can read it concurrently.
///
/// Each thread opens its provider with `make_state_provider` on its first read.
pub(crate) struct SharedStateProviderDatabase<'a> {
    make_state_provider: &'a (dyn Fn() -> ProviderResult<EvmStateProviderBox> + Sync),
    providers: ThreadLocal<RefCell<StateProviderDatabase<EvmStateProviderBox>>>,
}

impl<'a> SharedStateProviderDatabase<'a> {
    /// Creates a database that opens each thread's provider with `make_state_provider`.
    pub(crate) fn new(
        make_state_provider: &'a (dyn Fn() -> ProviderResult<EvmStateProviderBox> + Sync),
    ) -> Self {
        Self { make_state_provider, providers: ThreadLocal::new() }
    }

    /// Runs `f` with the calling thread's provider.
    fn with_provider<T>(
        &self,
        f: impl FnOnce(&mut StateProviderDatabase<EvmStateProviderBox>) -> ProviderResult<T>,
    ) -> ProviderResult<T> {
        let provider = self.providers.get_or_try(|| {
            (self.make_state_provider)()
                .map(|provider| RefCell::new(StateProviderDatabase::new(provider)))
        })?;
        f(&mut provider.borrow_mut())
    }
}

impl core::fmt::Debug for SharedStateProviderDatabase<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedStateProviderDatabase").finish_non_exhaustive()
    }
}

impl DatabaseRef for SharedStateProviderDatabase<'_> {
    type Error = ProviderError;

    fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.with_provider(|db| db.get_account(address))
    }

    fn get_code_by_hash(&self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        self.with_provider(|db| db.get_code_by_hash(code_hash))
    }

    fn get_storage(&self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        self.with_provider(|db| db.get_storage(address, key))
    }

    fn get_block_hash(&self, number: &Word) -> Result<B256, Self::Error> {
        self.with_provider(|db| db.get_block_hash(number))
    }
}
