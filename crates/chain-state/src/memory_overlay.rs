use super::ExecutedBlock;
use reth_errors::ProviderResult;
use reth_primitives::{
    keccak256, Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, B256,
};
use reth_storage_api::{
    AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateProviderBox,
    StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdates, AccountProof, HashedPostState,
    HashedStorage,
};
use std::{collections::HashMap, sync::OnceLock};

/// A state provider that stores references to in-memory blocks along with their state as well as
/// the historical state provider for fallback lookups.
#[allow(missing_debug_implementations)]
pub struct MemoryOverlayStateProvider {
    /// Historical state provider for state lookups that are not found in in-memory blocks.
    pub(crate) historical: Box<dyn StateProvider>,
    /// The collection of executed parent blocks. Expected order is newest to oldest.
    pub(crate) in_memory: Vec<ExecutedBlock>,
    /// Lazy-loaded in-memory trie data.
    pub(crate) trie_state: OnceLock<MemoryOverlayTrieState>,
}

impl MemoryOverlayStateProvider {
    /// Create new memory overlay state provider.
    ///
    /// ## Arguments
    ///
    /// - `in_memory` - the collection of executed ancestor blocks in reverse.
    /// - `historical` - a historical state provider for the latest ancestor block stored in the
    ///   database.
    pub fn new(historical: Box<dyn StateProvider>, in_memory: Vec<ExecutedBlock>) -> Self {
        Self { historical, in_memory, trie_state: OnceLock::new() }
    }

    /// Turn this state provider into a [`StateProviderBox`]
    pub fn boxed(self) -> StateProviderBox {
        Box::new(self)
    }

    /// Return lazy-loaded trie state aggregated from in-memory blocks.
    fn trie_state(&self) -> &MemoryOverlayTrieState {
        self.trie_state.get_or_init(|| {
            let mut hashed_state = HashedPostState::default();
            let mut trie_nodes = TrieUpdates::default();
            for block in self.in_memory.iter().rev() {
                hashed_state.extend_ref(block.hashed_state.as_ref());
                trie_nodes.extend_ref(block.trie.as_ref());
            }
            MemoryOverlayTrieState { trie_nodes, hashed_state }
        })
    }
}

impl BlockHashReader for MemoryOverlayStateProvider {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        for block in &self.in_memory {
            if block.block.number == number {
                return Ok(Some(block.block.hash()))
            }
        }

        self.historical.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let range = start..end;
        let mut earliest_block_number = None;
        let mut in_memory_hashes = Vec::new();
        for block in &self.in_memory {
            if range.contains(&block.block.number) {
                in_memory_hashes.insert(0, block.block.hash());
                earliest_block_number = Some(block.block.number);
            }
        }

        let mut hashes =
            self.historical.canonical_hashes_range(start, earliest_block_number.unwrap_or(end))?;
        hashes.append(&mut in_memory_hashes);
        Ok(hashes)
    }
}

impl AccountReader for MemoryOverlayStateProvider {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        for block in &self.in_memory {
            if let Some(account) = block.execution_output.account(&address) {
                return Ok(account)
            }
        }

        self.historical.basic_account(address)
    }
}

impl StateRootProvider for MemoryOverlayStateProvider {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let prefix_sets = hashed_state.construct_prefix_sets();
        self.state_root_from_nodes(TrieUpdates::default(), hashed_state, prefix_sets)
    }

    fn state_root_from_nodes(
        &self,
        nodes: TrieUpdates,
        state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<B256> {
        let MemoryOverlayTrieState { mut trie_nodes, mut hashed_state } = self.trie_state().clone();
        trie_nodes.extend(nodes);
        hashed_state.extend(state);
        self.historical.state_root_from_nodes(trie_nodes, hashed_state, prefix_sets)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let prefix_sets = hashed_state.construct_prefix_sets();
        self.state_root_from_nodes_with_updates(TrieUpdates::default(), hashed_state, prefix_sets)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        nodes: TrieUpdates,
        state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let MemoryOverlayTrieState { mut trie_nodes, mut hashed_state } = self.trie_state().clone();
        trie_nodes.extend(nodes);
        hashed_state.extend(state);
        self.historical.state_root_from_nodes_with_updates(trie_nodes, hashed_state, prefix_sets)
    }
}

impl StorageRootProvider for MemoryOverlayStateProvider {
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        let mut hashed_storage = self
            .trie_state()
            .hashed_state
            .storages
            .get(&keccak256(address))
            .cloned()
            .unwrap_or_default();
        hashed_storage.extend(&storage);
        self.historical.storage_root(address, hashed_storage)
    }
}

impl StateProofProvider for MemoryOverlayStateProvider {
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn proof(
        &self,
        state: HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let mut hashed_state = self.trie_state().hashed_state.clone();
        hashed_state.extend(state);
        self.historical.proof(hashed_state, address, slots)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn witness(
        &self,
        overlay: HashedPostState,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        let mut hashed_state = self.trie_state().hashed_state.clone();
        hashed_state.extend(overlay);
        self.historical.witness(hashed_state, target)
    }
}

impl StateProvider for MemoryOverlayStateProvider {
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        for block in &self.in_memory {
            if let Some(value) = block.execution_output.storage(&address, storage_key.into()) {
                return Ok(Some(value))
            }
        }

        self.historical.storage(address, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        for block in &self.in_memory {
            if let Some(contract) = block.execution_output.bytecode(&code_hash) {
                return Ok(Some(contract))
            }
        }

        self.historical.bytecode_by_hash(code_hash)
    }
}

/// The collection of data necessary for trie-related operations for [`MemoryOverlayStateProvider`].
#[derive(Clone, Debug)]
pub(crate) struct MemoryOverlayTrieState {
    /// The collection of aggregated in-memory trie updates.
    pub(crate) trie_nodes: TrieUpdates,
    /// The collection of hashed state from in-memory blocks.
    pub(crate) hashed_state: HashedPostState,
}
