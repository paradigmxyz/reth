use std::collections::HashMap;

use super::ExecutedBlock;
use reth_errors::ProviderResult;
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, B256,
};
use reth_storage_api::{
    AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateProviderBox,
    StateRootProvider,
};
use reth_trie::{updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage};

/// A state provider that stores references to in-memory blocks along with their state as well as
/// the historical state provider for fallback lookups.
#[allow(missing_debug_implementations)]
pub struct MemoryOverlayStateProvider {
    /// The collection of executed parent blocks. Expected order is newest to oldest.
    pub(crate) in_memory: Vec<ExecutedBlock>,
    /// The collection of hashed state from in-memory blocks.
    pub(crate) hashed_post_state: HashedPostState,
    /// Historical state provider for state lookups that are not found in in-memory blocks.
    pub(crate) historical: Box<dyn StateProvider>,
}

impl MemoryOverlayStateProvider {
    /// Create new memory overlay state provider.
    ///
    /// ## Arguments
    ///
    /// - `in_memory` - the collection of executed ancestor blocks in reverse.
    /// - `historical` - a historical state provider for the latest ancestor block stored in the
    ///   database.
    pub fn new(in_memory: Vec<ExecutedBlock>, historical: Box<dyn StateProvider>) -> Self {
        let mut hashed_post_state = HashedPostState::default();
        for block in in_memory.iter().rev() {
            hashed_post_state.extend(block.hashed_state.as_ref().clone());
        }
        Self { in_memory, hashed_post_state, historical }
    }

    /// Turn this state provider into a [`StateProviderBox`]
    pub fn boxed(self) -> StateProviderBox {
        Box::new(self)
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
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state);
        self.historical.hashed_state_root(state)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state);
        self.historical.hashed_state_root_with_updates(state)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_storage_root(
        &self,
        address: Address,
        storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.historical.hashed_storage_root(address, storage)
    }
}

impl StateProofProvider for MemoryOverlayStateProvider {
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_proof(
        &self,
        hashed_state: HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state);
        self.historical.hashed_proof(state, address, slots)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn witness(
        &self,
        overlay: HashedPostState,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        let mut state = self.hashed_post_state.clone();
        state.extend(overlay);
        self.historical.witness(state, target)
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
