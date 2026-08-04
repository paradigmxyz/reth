use super::ExecutedBlock;
use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode, NodePrimitives};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateProviderBox, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::{StorageTrieUpdatesSorted, TrieUpdates},
    AccountProof, HashedPostState, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
    TrieInput,
};
use revm::database::BundleState;
use std::{borrow::Cow, sync::OnceLock};

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
    pub(crate) in_memory: Cow<'a, [ExecutedBlock<N>]>,
    /// Lazy-loaded in-memory trie data.
    pub(crate) trie_input: OnceLock<TrieInput>,
}

impl<'a, N: NodePrimitives> MemoryOverlayStateProviderRef<'a, N> {
    /// Create new memory overlay state provider.
    ///
    /// ## Arguments
    ///
    /// - `in_memory` - the collection of executed ancestor blocks in reverse.
    /// - `historical` - a historical state provider for the latest ancestor block stored in the
    ///   database.
    pub fn new(historical: Box<dyn StateProvider + 'a>, in_memory: Vec<ExecutedBlock<N>>) -> Self {
        Self { historical, in_memory: Cow::Owned(in_memory), trie_input: OnceLock::new() }
    }

    /// Turn this state provider into a state provider
    pub fn boxed(self) -> Box<dyn StateProvider + 'a> {
        Box::new(self)
    }

    /// Return lazy-loaded trie state aggregated from in-memory blocks.
    fn trie_input(&self) -> &TrieInput {
        self.trie_input.get_or_init(|| {
            let mut input = TrieInput::default();
            // Iterate from oldest to newest
            for block in self.in_memory.iter().rev() {
                let data = block.trie_data();
                input.nodes.extend_from_sorted(&data.sorted.trie_updates);
                input.state.extend_from_sorted(&data.sorted.hashed_state);
            }
            input
        })
    }

    fn merged_hashed_storage(&self, address: Address, storage: HashedStorage) -> HashedStorage {
        let state = &self.trie_input().state;
        let mut hashed = state.storages.get(&keccak256(address)).cloned().unwrap_or_default();
        hashed.extend(&storage);
        hashed
    }

    fn storage_trie_nodes(&self, address: Address) -> Option<StorageTrieUpdatesSorted> {
        self.trie_input()
            .nodes
            .storage_tries_ref()
            .get(&keccak256(address))
            .map(|nodes| nodes.clone_into_sorted())
    }

    fn merged_storage_trie_nodes(
        &self,
        address: Address,
        nodes: Option<&StorageTrieUpdatesSorted>,
    ) -> Option<StorageTrieUpdatesSorted> {
        let mut merged = self.storage_trie_nodes(address);
        match (merged.as_mut(), nodes) {
            (Some(merged), Some(nodes)) => merged.extend_ref(nodes),
            (None, Some(nodes)) => return Some(nodes.clone()),
            _ => {}
        }
        merged
    }
}

impl<N: NodePrimitives> BlockHashReader for MemoryOverlayStateProviderRef<'_, N> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        for block in self.in_memory.iter() {
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
        for block in self.in_memory.iter() {
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
        for block in self.in_memory.iter() {
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
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, None) {
            self.historical.storage_root_from_nodes(address, merged, &nodes)
        } else {
            self.historical.storage_root(address, merged)
        }
    }

    fn storage_root_from_nodes(
        &self,
        address: Address,
        storage: HashedStorage,
        nodes: &StorageTrieUpdatesSorted,
    ) -> ProviderResult<B256> {
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, Some(nodes)) {
            self.historical.storage_root_from_nodes(address, merged, &nodes)
        } else {
            self.historical.storage_root(address, merged)
        }
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, None) {
            self.historical.storage_proof_from_nodes(address, slot, merged, &nodes)
        } else {
            self.historical.storage_proof(address, slot, merged)
        }
    }

    fn storage_proof_from_nodes(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
        nodes: &StorageTrieUpdatesSorted,
    ) -> ProviderResult<reth_trie::StorageProof> {
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, Some(nodes)) {
            self.historical.storage_proof_from_nodes(address, slot, merged, &nodes)
        } else {
            self.historical.storage_proof(address, slot, merged)
        }
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, None) {
            self.historical.storage_multiproof_from_nodes(address, slots, merged, &nodes)
        } else {
            self.historical.storage_multiproof(address, slots, merged)
        }
    }

    fn storage_multiproof_from_nodes(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
        nodes: &StorageTrieUpdatesSorted,
    ) -> ProviderResult<StorageMultiProof> {
        let merged = self.merged_hashed_storage(address, storage);
        if let Some(nodes) = self.merged_storage_trie_nodes(address, Some(nodes)) {
            self.historical.storage_multiproof_from_nodes(address, slots, merged, &nodes)
        } else {
            self.historical.storage_multiproof(address, slots, merged)
        }
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

    fn witness(
        &self,
        mut input: TrieInput,
        target: HashedPostState,
        mode: reth_trie::ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        input.prepend_self(self.trie_input().clone());
        self.historical.witness(input, target, mode)
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
        for block in self.in_memory.iter() {
            if let Some(value) = block.execution_output.storage(&address, storage_key.into()) {
                return Ok(Some(value));
            }
        }

        self.historical.storage(address, storage_key)
    }
}

impl<N: NodePrimitives> BytecodeReader for MemoryOverlayStateProviderRef<'_, N> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        for block in self.in_memory.iter() {
            if let Some(contract) = block.execution_output.bytecode(code_hash) {
                return Ok(Some(contract));
            }
        }

        self.historical.bytecode_by_hash(code_hash)
    }
}

/// An owned state provider that stores references to in-memory blocks along with their state as
/// well as a reference of the historical state provider for fallback lookups.
#[expect(missing_debug_implementations)]
pub struct MemoryOverlayStateProvider<N: NodePrimitives = reth_ethereum_primitives::EthPrimitives> {
    /// Historical state provider for state lookups that are not found in memory blocks.
    pub(crate) historical: StateProviderBox,
    /// The collection of executed parent blocks. Expected order is newest to oldest.
    pub(crate) in_memory: Vec<ExecutedBlock<N>>,
    /// Lazy-loaded in-memory trie data.
    pub(crate) trie_input: OnceLock<TrieInput>,
}

impl<N: NodePrimitives> MemoryOverlayStateProvider<N> {
    /// Create new memory overlay state provider.
    ///
    /// ## Arguments
    ///
    /// - `in_memory` - the collection of executed ancestor blocks in reverse.
    /// - `historical` - a historical state provider for the latest ancestor block stored in the
    ///   database.
    pub fn new(historical: StateProviderBox, in_memory: Vec<ExecutedBlock<N>>) -> Self {
        Self { historical, in_memory, trie_input: OnceLock::new() }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> MemoryOverlayStateProviderRef<'_, N> {
        MemoryOverlayStateProviderRef {
            historical: Box::new(self.historical.as_ref()),
            in_memory: Cow::Borrowed(&self.in_memory),
            trie_input: self.trie_input.clone(),
        }
    }

    /// Wraps the [`Self`] in a `Box`.
    pub fn boxed(self) -> StateProviderBox {
        Box::new(self)
    }
}

// Delegates all provider impls to [`MemoryOverlayStateProviderRef`]
reth_storage_api::macros::delegate_provider_impls!(MemoryOverlayStateProvider<N> where [N: NodePrimitives]);

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_trie::{
        updates::StorageTrieUpdates, ComputedTrieData, ExecutionWitnessMode, HashedPostStateSorted,
        LazyTrieData, StorageProof,
    };
    use std::sync::Arc;

    /// Sentinel returned by the fallback (`storage_root`) path.
    fn plain_root() -> B256 {
        B256::repeat_byte(0x11)
    }
    /// Sentinel returned by the node-aware (`storage_root_from_nodes`) path.
    fn node_aware_root() -> B256 {
        B256::repeat_byte(0x22)
    }

    /// Historical provider whose two storage-root paths return distinct sentinels, so a test can
    /// prove which one [`MemoryOverlayStateProviderRef`] dispatched to.
    struct DispatchMock;

    impl StateProvider for DispatchMock {
        fn storage(
            &self,
            _address: Address,
            _storage_key: StorageKey,
        ) -> ProviderResult<Option<StorageValue>> {
            Ok(None)
        }
    }

    impl BytecodeReader for DispatchMock {
        fn bytecode_by_hash(&self, _code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
            Ok(None)
        }
    }

    impl BlockHashReader for DispatchMock {
        fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(
            &self,
            _start: BlockNumber,
            _end: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok(vec![])
        }
    }

    impl AccountReader for DispatchMock {
        fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
            Ok(None)
        }
    }

    impl StateRootProvider for DispatchMock {
        fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_with_updates(
            &self,
            _hashed_state: HashedPostState,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }

        fn state_root_from_nodes_with_updates(
            &self,
            _input: TrieInput,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }
    }

    impl HashedPostStateProvider for DispatchMock {
        fn hashed_post_state(&self, _bundle_state: &BundleState) -> HashedPostState {
            HashedPostState::default()
        }
    }

    impl StorageRootProvider for DispatchMock {
        fn storage_root(
            &self,
            _address: Address,
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<B256> {
            Ok(plain_root())
        }

        fn storage_proof(
            &self,
            _address: Address,
            slot: B256,
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageProof> {
            Ok(StorageProof::new(slot))
        }

        fn storage_multiproof(
            &self,
            _address: Address,
            _slots: &[B256],
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageMultiProof> {
            Ok(StorageMultiProof::empty())
        }

        // The overlay must call this (not `storage_root`) when it has in-memory storage nodes.
        fn storage_root_from_nodes(
            &self,
            _address: Address,
            _hashed_storage: HashedStorage,
            _nodes: &StorageTrieUpdatesSorted,
        ) -> ProviderResult<B256> {
            Ok(node_aware_root())
        }
    }

    impl StateProofProvider for DispatchMock {
        fn proof(
            &self,
            _input: TrieInput,
            _address: Address,
            _slots: &[B256],
        ) -> ProviderResult<AccountProof> {
            Ok(AccountProof::new(Address::ZERO))
        }

        fn multiproof(
            &self,
            _input: TrieInput,
            _targets: MultiProofTargets,
        ) -> ProviderResult<MultiProof> {
            Ok(MultiProof::default())
        }

        fn witness(
            &self,
            _input: TrieInput,
            _target: HashedPostState,
            _mode: ExecutionWitnessMode,
        ) -> ProviderResult<Vec<Bytes>> {
            Ok(Vec::default())
        }
    }

    /// Builds an in-memory executed block whose trie data carries a storage-trie entry for
    /// `address`, giving the overlay in-memory storage nodes to reuse for that account.
    fn block_with_storage_nodes(address: Address) -> ExecutedBlock<EthPrimitives> {
        let mut nodes = TrieUpdates::default();
        nodes.storage_tries.insert(keccak256(address), StorageTrieUpdates::default());
        let trie_data = ComputedTrieData::new(
            Arc::new(HashedPostStateSorted::default()),
            Arc::new(nodes.into_sorted()),
        );
        ExecutedBlock::<EthPrimitives> {
            trie_data: LazyTrieData::ready(trie_data),
            ..Default::default()
        }
    }

    #[test]
    fn storage_root_dispatches_to_node_aware_path_when_in_memory_nodes_exist() {
        let with_nodes = Address::from([0xAA; 20]);
        let without_nodes = Address::from([0xBB; 20]);

        let overlay = MemoryOverlayStateProviderRef::<EthPrimitives>::new(
            Box::new(DispatchMock),
            vec![block_with_storage_nodes(with_nodes)],
        );

        // Account WITH in-memory storage nodes -> node-aware fast path.
        assert_eq!(
            overlay.storage_root(with_nodes, HashedStorage::default()).unwrap(),
            node_aware_root(),
            "overlay should dispatch into storage_root_from_nodes when in-memory nodes exist",
        );

        // Account WITHOUT in-memory storage nodes -> fallback to the plain path.
        assert_eq!(
            overlay.storage_root(without_nodes, HashedStorage::default()).unwrap(),
            plain_root(),
            "overlay should fall back to storage_root when no in-memory nodes exist",
        );
    }
}
