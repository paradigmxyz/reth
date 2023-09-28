use crate::{
    bundle_state::BundleStateWithReceipts, AccountReader, BlockHashReader, BundleStateDataProvider,
    StateProvider, StateRootProvider,
};
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_primitives::{Account, Address, BlockNumber, Bytecode, Bytes, B256};

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
    fn block_hash(&self, block_number: BlockNumber) -> RethResult<Option<B256>> {
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
    ) -> RethResult<Vec<B256>> {
        unimplemented!()
    }
}

impl<SP: StateProvider, BSDP: BundleStateDataProvider> AccountReader
    for BundleStateProvider<SP, BSDP>
{
    fn basic_account(&self, address: Address) -> RethResult<Option<Account>> {
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
    fn state_root(&self, post_state: &BundleStateWithReceipts) -> RethResult<B256> {
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
    ) -> RethResult<Option<reth_primitives::StorageValue>> {
        let u256_storage_key = storage_key.into();
        if let Some(value) =
            self.post_state_data_provider.state().storage(&account, u256_storage_key)
        {
            return Ok(Some(value))
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> RethResult<Option<Bytecode>> {
        if let Some(bytecode) = self.post_state_data_provider.state().bytecode(&code_hash) {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[B256],
    ) -> RethResult<(Vec<Bytes>, B256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock.into())
    }
}
