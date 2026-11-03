//! In-memory [`StateBackend`] for tests.

use alloy_primitives::{map::B256Map, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_api::StateBackend;
use reth_storage_errors::provider::ProviderResult;

/// [`StateBackend`] that serves current state from in-memory maps.
#[derive(Debug, Default)]
pub struct InMemoryStateBackend {
    /// Accounts by hashed address.
    pub accounts: B256Map<Account>,
    /// Storage values by hashed address, then hashed slot.
    pub storages: B256Map<B256Map<U256>>,
}

impl StateBackend for InMemoryStateBackend {
    fn account(&self, hashed_address: B256) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.get(&hashed_address).cloned())
    }

    fn storage(&self, hashed_address: B256, hashed_slot: B256) -> ProviderResult<Option<U256>> {
        Ok(self
            .storages
            .get(&hashed_address)
            .and_then(|storage| storage.get(&hashed_slot))
            .copied())
    }
}
