use super::ExecutedBlock;
use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode, NodePrimitives};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use revm_database::BundleState;
use std::sync::OnceLock;

/// A state provider that stores references to in-memory blocks along with their state as well as a
/// reference of the historical state provider for fallback lookups.
#[expect(missing_debug_implementations)]
pub struct MemoryOverlayStateProviderRef<
    'a,
    N: NodePrimitives = reth_ethereum_primitives::EthPrimitives,
> {
    /// Historical state provider for state lookups that are not found in memory blocks.
    pub(crate) historical: Box<dyn StateProvider + 'a>,
    /// The collection of executed parent blocks. Expected order is newest to oldest.
    pub(crate) in_memory: Vec<ExecutedBlock<N>>,
    /// Lazy-loaded in-memory trie data.
    pub(crate) trie_input: OnceLock<TrieInput>,
}

/// A state provider that stores references to in-memory blocks along with their state as well as
/// the historical state provider for fallback lookups.
pub type MemoryOverlayStateProvider<N> = MemoryOverlayStateProviderRef<'static, N>;

impl<'a, N: NodePrimitives> MemoryOverlayStateProviderRef<'a, N> {
    /// Create new memory overlay state provider.
    ///
    /// ## Arguments
    ///
    /// - `in_memory` - the collection of executed ancestor blocks in reverse.
    /// - `historical` - a historical state provider for the latest ancestor block stored in the
    ///   database.
    pub fn new(historical: Box<dyn StateProvider + 'a>, in_memory: Vec<ExecutedBlock<N>>) -> Self {
        Self { historical, in_memory, trie_input: OnceLock::new() }
    }

    /// Turn this state provider into a state provider
    pub fn boxed(self) -> Box<dyn StateProvider + 'a> {
        Box::new(self)
    }

    /// Return lazy-loaded trie state aggregated from in-memory blocks.
    fn trie_input(&self) -> &TrieInput {
        self.trie_input.get_or_init(|| {
            let bundles: Vec<_> =
                self.in_memory.iter().rev().map(|block| block.trie_data()).collect();
            TrieInput::from_blocks_sorted(
                bundles.iter().map(|data| (data.hashed_state.as_ref(), data.trie_updates.as_ref())),
            )
        })
    }
    fn merged_hashed_storage(&self, address: Address, storage: HashedStorage) -> HashedStorage {
        let state = &self.trie_input().state;
        let mut hashed = state.storages.get(&keccak256(address)).cloned().unwrap_or_default();
        hashed.extend(&storage);
        hashed
    }
}

impl<N: NodePrimitives> BlockHashReader for MemoryOverlayStateProviderRef<'_, N> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        for block in &self.in_memory {
            if block.recovered_block().number() == number {
                return Ok(Some(block.recovered_block().hash()));
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
        let mut in_memory_hashes = Vec::with_capacity(range.size_hint().0);

        // iterate in ascending order (oldest to newest = low to high)
        for block in &self.in_memory {
            let block_num = block.recovered_block().number();
            if range.contains(&block_num) {
                in_memory_hashes.push(block.recovered_block().hash());
                earliest_block_number = Some(block_num);
            }
        }

        // `self.in_memory` stores executed blocks in ascending order (oldest to newest).
        // However, `in_memory_hashes` should be constructed in descending order (newest to oldest),
        // so we reverse the vector after collecting the hashes.
        in_memory_hashes.reverse();

        let mut hashes =
            self.historical.canonical_hashes_range(start, earliest_block_number.unwrap_or(end))?;
        hashes.append(&mut in_memory_hashes);
        Ok(hashes)
    }
}

impl<N: NodePrimitives> AccountReader for MemoryOverlayStateProviderRef<'_, N> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        for block in &self.in_memory {
            if let Some(account) = block.execution_output.account(address) {
                return Ok(account);
            }
        }

        self.historical.basic_account(address)
    }
}

impl<N: NodePrimitives> StateRootProvider for MemoryOverlayStateProviderRef<'_, N> {
    fn state_root(&self, state: HashedPostState) -> ProviderResult<B256> {
        self.state_root_from_nodes(TrieInput::from_state(state))
    }

    fn state_root_from_nodes(&self, mut input: TrieInput) -> ProviderResult<B256> {
        input.prepend_self(self.trie_input().clone());
        self.historical.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_root_from_nodes_with_updates(TrieInput::from_state(state))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        mut input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        input.prepend_self(self.trie_input().clone());
        self.historical.state_root_from_nodes_with_updates(input)
    }
}

impl<N: NodePrimitives> StorageRootProvider for MemoryOverlayStateProviderRef<'_, N> {
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        self.storage_root_from_nodes(
            address,
            TrieInput::from_hashed_storage(keccak256(address), storage),
        )
    }

    fn storage_root_from_nodes(
        &self,
        address: Address,
        mut input: TrieInput,
    ) -> ProviderResult<B256> {
        input.prepend_self(self.trie_input().clone());
        self.historical.storage_root_from_nodes(address, input)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.storage_proof_from_nodes(
            address,
            slot,
            TrieInput::from_hashed_storage(keccak256(address), storage),
        )
    }

    fn storage_proof_from_nodes(
        &self,
        address: Address,
        slot: B256,
        mut input: TrieInput,
    ) -> ProviderResult<StorageProof> {
        input.prepend_self(self.trie_input().clone());
        self.historical.storage_proof_from_nodes(address, slot, input)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.storage_multiproof_from_nodes(
            address,
            slots,
            TrieInput::from_hashed_storage(keccak256(address), storage),
        )
    }

    fn storage_multiproof_from_nodes(
        &self,
        address: Address,
        slots: &[B256],
        mut input: TrieInput,
    ) -> ProviderResult<StorageMultiProof> {
        input.prepend_self(self.trie_input().clone());
        self.historical.storage_multiproof_from_nodes(address, slots, input)
    }
}

impl<N: NodePrimitives> StateProofProvider for MemoryOverlayStateProviderRef<'_, N> {
    fn proof(
        &self,
        mut input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        input.prepend_self(self.trie_input().clone());
        self.historical.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        mut input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        input.prepend_self(self.trie_input().clone());
        self.historical.multiproof(input, targets)
    }

    fn witness(&self, mut input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        input.prepend_self(self.trie_input().clone());
        self.historical.witness(input, target)
    }
}

impl<N: NodePrimitives> HashedPostStateProvider for MemoryOverlayStateProviderRef<'_, N> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        self.historical.hashed_post_state(bundle_state)
    }
}

impl<N: NodePrimitives> StateProvider for MemoryOverlayStateProviderRef<'_, N> {
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        for block in &self.in_memory {
            if let Some(value) = block.execution_output.storage(&address, storage_key.into()) {
                return Ok(Some(value));
            }
        }

        self.historical.storage(address, storage_key)
    }
}

impl<N: NodePrimitives> BytecodeReader for MemoryOverlayStateProviderRef<'_, N> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        for block in &self.in_memory {
            if let Some(contract) = block.execution_output.bytecode(code_hash) {
                return Ok(Some(contract));
            }
        }

        self.historical.bytecode_by_hash(code_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComputedTrieData, ExecutedBlock};
    use alloy_primitives::{keccak256, map::HashMap, Address, U256};
    use reth_primitives_traits::RecoveredBlock;
    use reth_trie::{
        updates::{StorageTrieUpdates, TrieUpdates},
        BranchNodeCompact, HashedPostState, HashedStorage, Nibbles, TrieInput,
    };
    use std::sync::Arc;

    /// Helper to create an executed block with specific storage state and trie updates.
    fn create_executed_block_with_storage(
        hashed_address: B256,
        storage: HashedStorage,
        trie_updates: TrieUpdates,
    ) -> ExecutedBlock {
        let trie_updates_sorted = trie_updates.into_sorted();
        let mut hashed_state = HashedPostState::default();
        hashed_state.storages.insert(hashed_address, storage);
        let hashed_state_sorted = hashed_state.into_sorted();

        let computed_trie_data = ComputedTrieData {
            hashed_state: Arc::new(hashed_state_sorted),
            trie_updates: Arc::new(trie_updates_sorted),
            anchored_trie_input: None,
        };

        ExecutedBlock::new(
            Arc::new(RecoveredBlock::default()),
            Arc::new(Default::default()),
            computed_trie_data,
        )
    }

    /// Tests that trie input correctly aggregates data from multiple in-memory blocks
    /// and that newer blocks' data takes precedence over older blocks' data.
    #[test]
    fn test_trie_input_aggregates_multiple_blocks_with_correct_precedence() {
        let address = Address::random();
        let hashed_address = keccak256(address);

        // Slot keys
        let slot1 = keccak256(B256::ZERO);
        let slot2 = keccak256(B256::with_last_byte(1));
        let slot3 = keccak256(B256::with_last_byte(2));

        // Block 1 (older): slot1=100, slot2=200
        let mut storage1 = HashMap::default();
        storage1.insert(slot1, U256::from(100));
        storage1.insert(slot2, U256::from(200));
        let hashed_storage1 = HashedStorage { storage: storage1, wiped: false };

        let mut trie_updates1 = TrieUpdates::default();
        let mut storage_trie1 = StorageTrieUpdates::default();
        let path1 = Nibbles::from_nibbles([0x0, 0x1]);
        storage_trie1
            .storage_nodes
            .insert(path1, BranchNodeCompact::new(0b0001, 0b0000, 0, Vec::new(), None));
        trie_updates1.storage_tries.insert(hashed_address, storage_trie1);

        let block1 =
            create_executed_block_with_storage(hashed_address, hashed_storage1, trie_updates1);

        // Block 2 (newer): slot2=999 (overrides), slot3=300 (new)
        let mut storage2 = HashMap::default();
        storage2.insert(slot2, U256::from(999));
        storage2.insert(slot3, U256::from(300));
        let hashed_storage2 = HashedStorage { storage: storage2, wiped: false };

        let mut trie_updates2 = TrieUpdates::default();
        let mut storage_trie2 = StorageTrieUpdates::default();
        let path2 = Nibbles::from_nibbles([0x0, 0x2]);
        storage_trie2
            .storage_nodes
            .insert(path2, BranchNodeCompact::new(0b0010, 0b0000, 0, Vec::new(), None));
        trie_updates2.storage_tries.insert(hashed_address, storage_trie2);

        let block2 =
            create_executed_block_with_storage(hashed_address, hashed_storage2, trie_updates2);

        // Create provider with blocks in newest-to-oldest order (as expected by the provider)
        // in_memory stores blocks newest first, but trie_input() reverses them
        let in_memory = vec![block2, block1];
        let historical = Box::new(reth_storage_api::noop::NoopProvider::default());
        let provider = MemoryOverlayStateProviderRef::new(historical, in_memory);

        // Get the aggregated trie input
        let trie_input = provider.trie_input();

        // Verify trie nodes from both blocks are present
        let storage_trie = trie_input
            .nodes
            .storage_tries
            .get(&hashed_address)
            .expect("storage trie updates should exist");
        assert!(
            storage_trie.storage_nodes.contains_key(&Nibbles::from_nibbles([0x0, 0x1])),
            "trie node from block1 should be present"
        );
        assert!(
            storage_trie.storage_nodes.contains_key(&Nibbles::from_nibbles([0x0, 0x2])),
            "trie node from block2 should be present"
        );

        // Verify storage state has correct values (newer block overrides)
        let storage =
            trie_input.state.storages.get(&hashed_address).expect("storage state should exist");

        assert_eq!(
            storage.storage.get(&slot1),
            Some(&U256::from(100)),
            "slot1 from block1 should be present"
        );
        assert_eq!(
            storage.storage.get(&slot2),
            Some(&U256::from(999)),
            "slot2 should have block2's value (999), not block1's (200)"
        );
        assert_eq!(
            storage.storage.get(&slot3),
            Some(&U256::from(300)),
            "slot3 from block2 should be present"
        );

        // Now test that prepend_self correctly layers new input on top
        let slot4 = keccak256(B256::with_last_byte(3));
        let mut new_storage = HashMap::default();
        new_storage.insert(slot1, U256::from(9999)); // Override slot1 again
        new_storage.insert(slot4, U256::from(400)); // New slot4
        let new_hashed_storage = HashedStorage { storage: new_storage, wiped: false };

        let mut input = TrieInput::from_hashed_storage(hashed_address, new_hashed_storage);
        input.prepend_self(trie_input.clone());

        // After prepend_self, input should have:
        // - All trie nodes from in-memory blocks
        // - slot1=9999 (from new input, overriding in-memory)
        // - slot2=999 (from in-memory block2)
        // - slot3=300 (from in-memory block2)
        // - slot4=400 (from new input)
        let final_storage =
            input.state.storages.get(&hashed_address).expect("final storage should exist");

        assert_eq!(
            final_storage.storage.get(&slot1),
            Some(&U256::from(9999)),
            "slot1 should be overridden by new input"
        );
        assert_eq!(
            final_storage.storage.get(&slot2),
            Some(&U256::from(999)),
            "slot2 should retain in-memory value"
        );
        assert_eq!(
            final_storage.storage.get(&slot3),
            Some(&U256::from(300)),
            "slot3 should retain in-memory value"
        );
        assert_eq!(
            final_storage.storage.get(&slot4),
            Some(&U256::from(400)),
            "slot4 from new input should be present"
        );

        // Verify trie nodes are also preserved
        let final_trie = input
            .nodes
            .storage_tries
            .get(&hashed_address)
            .expect("final trie updates should exist");
        assert_eq!(
            final_trie.storage_nodes.len(),
            2,
            "both trie nodes from in-memory blocks should be preserved"
        );
    }
}
