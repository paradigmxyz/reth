use crate::{
    AccountReader, BlockExecutionDataProvider, BlockHashReader, StateProvider, StateRootProvider,
};
use reth_primitives::{proofs::AccountProof, Account, Address, BlockNumber, Bytecode, B256};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::updates::TrieUpdates;
use revm::db::BundleState;

/// A state provider that resolves to data from either a wrapped [`crate::BlockExecutionOutcome`]
/// or an underlying state provider.
///
/// This struct combines two sources of state data: the block execution outcome and an underlying
/// state provider. It can provide state information by leveraging both the post-block execution
/// changes and the pre-existing state data.
#[derive(Debug)]
pub struct BundleStateProvider<SP: StateProvider, BEDP: BlockExecutionDataProvider> {
    /// The inner state provider.
    pub state_provider: SP,
    /// Block execution data.
    pub block_execution_data_provider: BEDP,
}

impl<SP: StateProvider, BEDP: BlockExecutionDataProvider> BundleStateProvider<SP, BEDP> {
    /// Create new bundle state provider
    pub const fn new(state_provider: SP, block_execution_data_provider: BEDP) -> Self {
        Self { state_provider, block_execution_data_provider }
    }
}

/* Implement StateProvider traits */

impl<SP: StateProvider, BEDP: BlockExecutionDataProvider> BlockHashReader
    for BundleStateProvider<SP, BEDP>
{
    fn block_hash(&self, block_number: BlockNumber) -> ProviderResult<Option<B256>> {
        let block_hash = self.block_execution_data_provider.block_hash(block_number);
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

impl<SP: StateProvider, BEDP: BlockExecutionDataProvider> AccountReader
    for BundleStateProvider<SP, BEDP>
{
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        if let Some(account) =
            self.block_execution_data_provider.block_execution_outcome().account(&address)
        {
            Ok(account)
        } else {
            self.state_provider.basic_account(address)
        }
    }
}

impl<SP: StateProvider, BEDP: BlockExecutionDataProvider> StateRootProvider
    for BundleStateProvider<SP, BEDP>
{
    fn state_root(&self, bundle_state: &BundleState) -> ProviderResult<B256> {
        let mut state =
            self.block_execution_data_provider.block_execution_outcome().state().clone();
        state.extend(bundle_state.clone());
        self.state_provider.state_root(&state)
    }

    fn state_root_with_updates(
        &self,
        bundle_state: &BundleState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let mut state =
            self.block_execution_data_provider.block_execution_outcome().state().clone();
        state.extend(bundle_state.clone());
        self.state_provider.state_root_with_updates(&state)
    }
}

impl<SP: StateProvider, BEDP: BlockExecutionDataProvider> StateProvider
    for BundleStateProvider<SP, BEDP>
{
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> ProviderResult<Option<reth_primitives::StorageValue>> {
        let u256_storage_key = storage_key.into();
        if let Some(value) = self
            .block_execution_data_provider
            .block_execution_outcome()
            .storage(&account, u256_storage_key)
        {
            return Ok(Some(value))
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(bytecode) =
            self.block_execution_data_provider.block_execution_outcome().bytecode(&code_hash)
        {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }

    fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock)
    }
}
