use crate::{
    AccountReader, BlockHashReader, ExecutionDataProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, BlockNumber, Bytes, B256,
};
use reth_primitives::{Account, Bytecode};
use reth_storage_api::{StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, TrieInput,
};

/// A state provider that resolves to data from either a wrapped [`crate::ExecutionOutcome`]
/// or an underlying state provider.
///
/// This struct combines two sources of state data: the execution outcome and an underlying
/// state provider. It can provide state information by leveraging both the post-block execution
/// changes and the pre-existing state data.
#[derive(Debug)]
pub struct BundleStateProvider<SP: StateProvider, EDP: ExecutionDataProvider> {
    /// The inner state provider.
    pub state_provider: SP,
    /// Block execution data.
    pub block_execution_data_provider: EDP,
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> BundleStateProvider<SP, EDP> {
    /// Create new bundle state provider
    pub const fn new(state_provider: SP, block_execution_data_provider: EDP) -> Self {
        Self { state_provider, block_execution_data_provider }
    }

    /// Retrieve hashed storage for target address.
    fn get_hashed_storage(&self, address: Address) -> HashedStorage {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        bundle_state
            .account(&address)
            .map(|account| {
                HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                )
            })
            .unwrap_or_else(|| HashedStorage::new(false))
    }
}

/* Implement StateProvider traits */

impl<SP: StateProvider, EDP: ExecutionDataProvider> BlockHashReader
    for BundleStateProvider<SP, EDP>
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

impl<SP: StateProvider, EDP: ExecutionDataProvider> AccountReader for BundleStateProvider<SP, EDP> {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        if let Some(account) =
            self.block_execution_data_provider.execution_outcome().account(&address)
        {
            Ok(account)
        } else {
            self.state_provider.basic_account(address)
        }
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateRootProvider
    for BundleStateProvider<SP, EDP>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state.state);
        state.extend(hashed_state);
        self.state_provider.state_root(state)
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        unimplemented!()
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state.state);
        state.extend(hashed_state);
        self.state_provider.state_root_with_updates(state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        mut input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        input.prepend(HashedPostState::from_bundle_state(&bundle_state.state));
        self.state_provider.state_root_from_nodes_with_updates(input)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StorageRootProvider
    for BundleStateProvider<SP, EDP>
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        let mut storage = self.get_hashed_storage(address);
        storage.extend(&hashed_storage);
        self.state_provider.storage_root(address, storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        let mut storage = self.get_hashed_storage(address);
        storage.extend(&hashed_storage);
        self.state_provider.storage_proof(address, slot, storage)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateProofProvider
    for BundleStateProvider<SP, EDP>
{
    fn proof(
        &self,
        mut input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        input.prepend(HashedPostState::from_bundle_state(&bundle_state.state));
        self.state_provider.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        mut input: reth_trie::TrieInput,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        input.prepend(HashedPostState::from_bundle_state(&bundle_state.state));
        self.state_provider.multiproof(input, targets)
    }

    fn witness(
        &self,
        mut input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        let bundle_state = self.block_execution_data_provider.execution_outcome().state();
        input.prepend(HashedPostState::from_bundle_state(&bundle_state.state));
        self.state_provider.witness(input, target)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateProvider for BundleStateProvider<SP, EDP> {
    fn storage(
        &self,
        account: Address,
        storage_key: alloy_primitives::StorageKey,
    ) -> ProviderResult<Option<alloy_primitives::StorageValue>> {
        let u256_storage_key = storage_key.into();
        if let Some(value) = self
            .block_execution_data_provider
            .execution_outcome()
            .storage(&account, u256_storage_key)
        {
            return Ok(Some(value))
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(bytecode) =
            self.block_execution_data_provider.execution_outcome().bytecode(&code_hash)
        {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }
}
