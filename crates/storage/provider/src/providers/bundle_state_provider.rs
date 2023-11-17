use crate::{
    bundle_state::BundleStateWithReceipts, AccountReader, BlockHashReader, BundleStateDataProvider,
    StateProvider, StateRootProvider,
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{trie::AccountProof, Account, Address, BlockNumber, Bytecode, B256};

/// A state provider that either resolves to data in a wrapped [`crate::BundleStateWithReceipts`],
/// or an underlying state provider.
#[derive(Debug)]
pub struct BundleStateProvider<SP: StateProvider, BSDP: BundleStateDataProvider> {
    /// The inner state provider.
    pub(crate) state_provider: SP,
    /// Post state data,
    pub(crate) post_state_data_provider: BSDP,
}

impl<SP: StateProvider, BSDP: BundleStateDataProvider> BundleStateProvider<SP, BSDP> {
    /// Create new post-state provider
    pub fn new(state_provider: SP, post_state_data_provider: BSDP) -> Self {
        Self { state_provider, post_state_data_provider }
    }
}

/* Implement StateProvider traits */

impl<SP: StateProvider, BSDP: BundleStateDataProvider> BlockHashReader
    for BundleStateProvider<SP, BSDP>
{
    fn block_hash(&self, block_number: BlockNumber) -> ProviderResult<Option<B256>> {
        let block_hash = self.post_state_data_provider.block_hash(block_number);
        if block_hash.is_some() {
            return Ok(block_hash)
        }
        self.state_provider.block_hash(block_number)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        unimplemented!()
    }
}

impl<SP: StateProvider, BSDP: BundleStateDataProvider> AccountReader
    for BundleStateProvider<SP, BSDP>
{
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        if let Some(account) = self.post_state_data_provider.state().account(&address) {
            Ok(account)
        } else {
            self.state_provider.basic_account(address)
        }
    }
}

impl<SP: StateProvider, BSDP: BundleStateDataProvider> StateRootProvider
    for BundleStateProvider<SP, BSDP>
{
    fn state_root(&self, post_state: &BundleStateWithReceipts) -> ProviderResult<B256> {
        let mut state = self.post_state_data_provider.state().clone();
        state.extend(post_state.clone());
        self.state_provider.state_root(&state)
    }
}

impl<SP: StateProvider, BSDP: BundleStateDataProvider> StateProvider
    for BundleStateProvider<SP, BSDP>
{
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> ProviderResult<Option<reth_primitives::StorageValue>> {
        let u256_storage_key = storage_key.into();
        if let Some(value) =
            self.post_state_data_provider.state().storage(&account, u256_storage_key)
        {
            return Ok(Some(value))
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(bytecode) = self.post_state_data_provider.state().bytecode(&code_hash) {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }

    fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock)
    }
}
