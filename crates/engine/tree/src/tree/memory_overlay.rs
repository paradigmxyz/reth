use super::ExecutedBlock;
use reth_errors::ProviderResult;
use reth_primitives::{Account, Address, BlockNumber, Bytecode, StorageKey, StorageValue, B256};
use reth_provider::{
    AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateRootProvider,
};
use reth_trie::{updates::TrieUpdates, AccountProof, HashedPostState};

/// A state provider that stores references to in-memory blocks along with their state as well as
/// the historical state provider for fallback lookups.
#[derive(Debug)]
pub struct MemoryOverlayStateProvider<H> {
    /// The collection of executed parent blocks.
    in_memory: Vec<ExecutedBlock>,
    /// The collection of hashed state from in-memory blocks.
    hashed_post_state: HashedPostState,
    /// Historical state provider for state lookups that are not found in in-memory blocks.
    historical: H,
}

impl<H> MemoryOverlayStateProvider<H> {
    /// Create new memory overlay state provider.
    pub fn new(in_memory: Vec<ExecutedBlock>, historical: H) -> Self {
        let mut hashed_post_state = HashedPostState::default();
        for block in &in_memory {
            hashed_post_state.extend(block.hashed_state.as_ref().clone());
        }
        Self { in_memory, hashed_post_state, historical }
    }
}

impl<H> BlockHashReader for MemoryOverlayStateProvider<H>
where
    H: BlockHashReader,
{
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        for block in self.in_memory.iter().rev() {
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
        for block in self.in_memory.iter().rev() {
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

impl<H> AccountReader for MemoryOverlayStateProvider<H>
where
    H: AccountReader + Send,
{
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        for block in self.in_memory.iter().rev() {
            if let Some(account) = block.execution_output.account(&address) {
                return Ok(account)
            }
        }

        self.historical.basic_account(address)
    }
}

impl<H> StateRootProvider for MemoryOverlayStateProvider<H>
where
    H: StateRootProvider + Send,
{
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_state_root(&self, hashed_state: &HashedPostState) -> ProviderResult<B256> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state.clone());
        self.historical.hashed_state_root(&state)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_state_root_with_updates(
        &self,
        hashed_state: &HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state.clone());
        self.historical.hashed_state_root_with_updates(&state)
    }
}

impl<H> StateProofProvider for MemoryOverlayStateProvider<H>
where
    H: StateProofProvider + Send,
{
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn hashed_proof(
        &self,
        hashed_state: &HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let mut state = self.hashed_post_state.clone();
        state.extend(hashed_state.clone());
        self.historical.hashed_proof(&state, address, slots)
    }
}

impl<H> StateProvider for MemoryOverlayStateProvider<H>
where
    H: StateProvider + Send,
{
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        for block in self.in_memory.iter().rev() {
            if let Some(value) = block.execution_output.storage(&address, storage_key.into()) {
                return Ok(Some(value))
            }
        }

        self.historical.storage(address, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        for block in self.in_memory.iter().rev() {
            if let Some(contract) = block.execution_output.bytecode(&code_hash) {
                return Ok(Some(contract))
            }
        }

        self.historical.bytecode_by_hash(code_hash)
    }
}
