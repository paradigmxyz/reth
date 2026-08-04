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
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, TrieInput,
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

    /// Consuming core of
    /// [`StateRootProvider::state_root_with_updates_consumed`](StateRootProvider): builds the
    /// cached input if it has not been built yet, then moves it into the computation instead
    /// of cloning it.
    fn consume_state_root_with_updates(
        self,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.trie_input();
        let Self { historical, trie_input, .. } = self;
        let mut input =
            trie_input.into_inner().expect("initialized by the call to trie_input above");
        input.append(state);
        historical.state_root_from_nodes_with_updates(input)
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

    fn state_root_with_updates_consumed(
        self: Box<Self>,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        (*self).consume_state_root_with_updates(state)
    }
}

impl<N: NodePrimitives> StorageRootProvider for MemoryOverlayStateProviderRef<'_, N> {
    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        let merged = self.merged_hashed_storage(address, storage);
        self.historical.storage_root(address, merged)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        let merged = self.merged_hashed_storage(address, storage);
        self.historical.storage_proof(address, slot, merged)
    }

    // TODO: Currently this does not reuse available in-memory trie nodes.
    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        let merged = self.merged_hashed_storage(address, storage);
        self.historical.storage_multiproof(address, slots, merged)
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

// Delegates all provider impls to [`MemoryOverlayStateProviderRef`], except
// `StateRootProvider`, which is implemented manually below to override the consuming
// state root variant.
reth_storage_api::macros::delegate_impls_to_as_ref!(
    for MemoryOverlayStateProvider<N> =>
    AccountReader where [N: NodePrimitives] {
        fn basic_account(&self, address: &alloy_primitives::Address) -> reth_storage_api::errors::provider::ProviderResult<Option<reth_primitives_traits::Account>>;
    }
    BlockHashReader where [N: NodePrimitives] {
        fn block_hash(&self, number: u64) -> reth_storage_api::errors::provider::ProviderResult<Option<alloy_primitives::B256>>;
        fn canonical_hashes_range(&self, start: alloy_primitives::BlockNumber, end: alloy_primitives::BlockNumber) -> reth_storage_api::errors::provider::ProviderResult<Vec<alloy_primitives::B256>>;
    }
    StateProvider where [N: NodePrimitives] {
        fn storage(&self, account: alloy_primitives::Address, storage_key: alloy_primitives::StorageKey) -> reth_storage_api::errors::provider::ProviderResult<Option<alloy_primitives::StorageValue>>;
    }
    BytecodeReader where [N: NodePrimitives] {
        fn bytecode_by_hash(&self, code_hash: &alloy_primitives::B256) -> reth_storage_api::errors::provider::ProviderResult<Option<reth_primitives_traits::Bytecode>>;
    }
    StorageRootProvider where [N: NodePrimitives] {
        fn storage_root(&self, address: alloy_primitives::Address, storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<alloy_primitives::B256>;
        fn storage_proof(&self, address: alloy_primitives::Address, slot: alloy_primitives::B256, storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::StorageProof>;
        fn storage_multiproof(&self, address: alloy_primitives::Address, slots: &[alloy_primitives::B256], storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::StorageMultiProof>;
    }
    StateProofProvider where [N: NodePrimitives] {
        fn proof(&self, input: reth_trie::TrieInput, address: alloy_primitives::Address, slots: &[alloy_primitives::B256]) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::AccountProof>;
        fn multiproof(&self, input: reth_trie::TrieInput, targets: reth_trie::MultiProofTargets) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::MultiProof>;
        fn witness(&self, input: reth_trie::TrieInput, target: reth_trie::HashedPostState, mode: reth_trie::ExecutionWitnessMode) -> reth_storage_api::errors::provider::ProviderResult<Vec<alloy_primitives::Bytes>>;
    }
    HashedPostStateProvider where [N: NodePrimitives] {
        fn hashed_post_state(&self, bundle_state: &revm::database::BundleState) -> reth_trie::HashedPostState;
    }
);

impl<N: NodePrimitives> StateRootProvider for MemoryOverlayStateProvider<N> {
    fn state_root(&self, state: HashedPostState) -> ProviderResult<B256> {
        self.as_ref().state_root(state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.as_ref().state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.as_ref().state_root_with_updates(state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.as_ref().state_root_from_nodes_with_updates(input)
    }

    fn state_root_with_updates_consumed(
        self: Box<Self>,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        // Move the historical provider, the blocks and the cached trie input into an
        // owning `Ref` instead of borrowing them and cloning the input through `as_ref`.
        let Self { historical, in_memory, trie_input } = *self;
        MemoryOverlayStateProviderRef { historical, in_memory: Cow::Owned(in_memory), trie_input }
            .consume_state_root_with_updates(state)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, EthPrimitives, ExecutedBlock};
    use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, U256};
    use reth_primitives_traits::{Account, Bytecode};
    use reth_storage_api::{
        AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider,
        StateProofProvider, StorageRootProvider,
    };
    use reth_trie::{
        prefix_set::PrefixSetMut, AccountProof, ComputedTrieData, ExecutionWitnessMode,
        HashedStorage, MultiProof, MultiProofTargets, Nibbles, StorageMultiProof, StorageProof,
    };
    use std::sync::{Arc, Mutex};

    /// Records the [`TrieInput`] received by `state_root_from_nodes_with_updates` and
    /// returns a root derived from it, so the borrowing and consuming call paths can be
    /// compared for equivalence.
    #[derive(Debug, Default)]
    struct RecordingStateProvider {
        received: Arc<Mutex<Option<TrieInput>>>,
    }

    impl AccountReader for RecordingStateProvider {
        fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
            Ok(None)
        }
    }

    impl BlockHashReader for RecordingStateProvider {
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

    impl BytecodeReader for RecordingStateProvider {
        fn bytecode_by_hash(&self, _code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
            Ok(None)
        }
    }

    impl StateRootProvider for RecordingStateProvider {
        fn state_root(&self, _state: HashedPostState) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_with_updates(
            &self,
            _state: HashedPostState,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }

        fn state_root_from_nodes_with_updates(
            &self,
            input: TrieInput,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            let root = B256::with_last_byte(input.state.accounts.len() as u8);
            let updates = input.nodes.clone();
            *self.received.lock().unwrap() = Some(input);
            Ok((root, updates))
        }
    }

    impl StorageRootProvider for RecordingStateProvider {
        fn storage_root(&self, _address: Address, _storage: HashedStorage) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn storage_proof(
            &self,
            _address: Address,
            slot: B256,
            _storage: HashedStorage,
        ) -> ProviderResult<StorageProof> {
            Ok(StorageProof::new(slot))
        }

        fn storage_multiproof(
            &self,
            _address: Address,
            _slots: &[B256],
            _storage: HashedStorage,
        ) -> ProviderResult<StorageMultiProof> {
            Ok(StorageMultiProof::empty())
        }
    }

    impl StateProofProvider for RecordingStateProvider {
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

    impl HashedPostStateProvider for RecordingStateProvider {
        fn hashed_post_state(
            &self,
            _bundle_state: &revm::database::BundleState,
        ) -> HashedPostState {
            HashedPostState::default()
        }
    }

    impl StateProvider for RecordingStateProvider {
        fn storage(
            &self,
            _account: Address,
            _storage_key: StorageKey,
        ) -> ProviderResult<Option<StorageValue>> {
            Ok(None)
        }
    }

    fn with_unique_state(
        block: &ExecutedBlock<EthPrimitives>,
        id: u8,
    ) -> ExecutedBlock<EthPrimitives> {
        let hashed_address = B256::with_last_byte(id);
        let hashed_slot = B256::with_last_byte(id.saturating_add(32));
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();

        let mut trie_updates = TrieUpdates::default();
        trie_updates.removed_nodes.insert(Nibbles::from_nibbles_unchecked([id & 0x0f]));

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(trie_updates.into_sorted())),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..4)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    fn overlay_state() -> HashedPostState {
        // Overlaps the first in-memory block account with a different value, so the
        // duplicate-precedence of the consuming path is compared against the borrowing
        // path, and adds a fresh account on top.
        let overlapping = Account { nonce: 7, ..Default::default() };
        HashedPostState::default()
            .with_accounts([
                (B256::with_last_byte(1), Some(overlapping)),
                (B256::with_last_byte(2), None),
                (B256::with_last_byte(0xAA), Some(Account::default())),
            ])
            .with_storages([(
                B256::with_last_byte(1),
                HashedStorage::from_iter(true, [(B256::with_last_byte(0x21), U256::from(9))]),
            )])
    }

    type CanonicalPrefixSet = (bool, Vec<Nibbles>);
    type CanonicalPrefixSets = (CanonicalPrefixSet, Vec<(B256, CanonicalPrefixSet)>, Vec<B256>);

    /// Prefix sets in their canonical (frozen) form, which is what consumers see: `freeze`
    /// sorts and deduplicates the keys and preserves the `all` flag. A wiped storage becomes
    /// an `all` set with no keys, which must not compare equal to an empty set, hence the
    /// flag is captured alongside the keys.
    fn canonical_prefix_set(set: &PrefixSetMut) -> CanonicalPrefixSet {
        let frozen = set.clone().freeze();
        (frozen.all(), frozen.slice().to_vec())
    }

    fn canonical_prefix_sets(input: &TrieInput) -> CanonicalPrefixSets {
        let sets = &input.prefix_sets;
        let mut storages: Vec<(B256, CanonicalPrefixSet)> = sets
            .storage_prefix_sets
            .iter()
            .map(|(address, set)| (*address, canonical_prefix_set(set)))
            .collect();
        storages.sort_unstable_by_key(|(address, _)| *address);
        let mut destroyed: Vec<_> = sets.destroyed_accounts.iter().copied().collect();
        destroyed.sort_unstable();
        (canonical_prefix_set(&sets.account_prefix_set), storages, destroyed)
    }

    #[test]
    fn consuming_state_root_matches_borrowing_path() {
        let blocks = test_blocks();

        let borrow_recorder = Arc::new(Mutex::new(None));
        let borrow_provider = MemoryOverlayStateProvider::<EthPrimitives>::new(
            Box::new(RecordingStateProvider { received: Arc::clone(&borrow_recorder) }),
            blocks.clone(),
        );
        let (borrowed_root, borrowed_updates) =
            borrow_provider.state_root_with_updates(overlay_state()).unwrap();

        let consume_recorder = Arc::new(Mutex::new(None));
        let consume_provider = MemoryOverlayStateProvider::<EthPrimitives>::new(
            Box::new(RecordingStateProvider { received: Arc::clone(&consume_recorder) }),
            blocks,
        )
        .boxed();
        let (consumed_root, consumed_updates) =
            consume_provider.state_root_with_updates_consumed(overlay_state()).unwrap();

        let borrowed_input = borrow_recorder.lock().unwrap().take().unwrap();
        let consumed_input = consume_recorder.lock().unwrap().take().unwrap();
        // The aggregated nodes are non-empty (block trie updates), so this comparison is
        // not vacuous.
        assert_ne!(borrowed_input.nodes, TrieUpdates::default());
        assert_eq!(borrowed_input.state, consumed_input.state);
        assert_eq!(borrowed_input.nodes, consumed_input.nodes);
        assert_eq!(canonical_prefix_sets(&borrowed_input), canonical_prefix_sets(&consumed_input));
        assert_eq!(borrowed_root, consumed_root);
        assert_eq!(borrowed_updates, consumed_updates);
    }

    #[test]
    fn consuming_state_root_works_through_trait_object() {
        let recorder = Arc::new(Mutex::new(None));
        let provider: Box<dyn StateProvider> = Box::new(MemoryOverlayStateProviderRef::new(
            Box::new(RecordingStateProvider { received: Arc::clone(&recorder) }),
            test_blocks(),
        ));

        let (root, _) = provider.state_root_with_updates_consumed(overlay_state()).unwrap();

        let input = recorder.lock().unwrap().take().unwrap();
        // 3 in-memory block accounts + the fresh overlay account; the overlapping account
        // carries the overlay value.
        assert_eq!(input.state.accounts.len(), 4);
        assert_eq!(
            input.state.accounts.get(&B256::with_last_byte(1)),
            Some(&Some(Account { nonce: 7, ..Default::default() }))
        );
        // The destroyed overlay account surfaces in destroyed_accounts, and the wiped
        // storage freezes to an `all` prefix set (not just a present, empty entry).
        assert!(input.prefix_sets.destroyed_accounts.contains(&B256::with_last_byte(2)));
        let wiped = input.prefix_sets.storage_prefix_sets.get(&B256::with_last_byte(1)).unwrap();
        assert!(wiped.clone().freeze().all());
        assert_eq!(root, B256::with_last_byte(4));
    }
}
