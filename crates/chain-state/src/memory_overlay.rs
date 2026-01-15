use super::ExecutedBlock;
use alloy_consensus::BlockHeader;
use alloy_primitives::{keccak256, Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode, NodePrimitives};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, PlainPostState,
    StateProofProvider, StateProvider, StateProviderBox, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, TrieInput,
};
use revm_database::BundleState;
use std::{borrow::Cow, sync::OnceLock, time::Instant};

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
                input.nodes.extend_from_sorted(&data.trie_updates);
                input.state.extend_from_sorted(&data.hashed_state);
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
        let start = Instant::now();
        let ret = self.historical.state_root_from_nodes_with_updates(input);
        let elapsed = start.elapsed().as_millis();
        tracing::info!(
            "MemoryOverlayStateProviderRef state_root_from_nodes_with_updates, elapsed={}ms",
            elapsed
        );
        ret
    }

    fn state_root_with_updates_plain(
        &self,
        plain_state: PlainPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        use alloy_primitives::map::HashMap;

        // Build PlainPostState from in-memory blocks
        let mut merged_state = PlainPostState::default();

        for block in self.in_memory.iter() {
            let bundle_state = &block.execution_output.bundle;
            for (address, bundle_account) in bundle_state.state() {
                let account = if bundle_account.was_destroyed() || bundle_account.info.is_none() {
                    None
                } else {
                    bundle_account
                        .info
                        .as_ref()
                        .map(|info| reth_primitives_traits::Account::from(info))
                };
                merged_state.accounts.insert(*address, account);

                let storage_map =
                    merged_state.storages.entry(*address).or_insert_with(HashMap::default);
                for (slot, storage_slot) in &bundle_account.storage {
                    let slot_b256 = B256::from_slice(&slot.to_be_bytes::<32>());
                    storage_map.insert(slot_b256, storage_slot.present_value);
                }
            }
        }

        // Merge with incoming plain_state (incoming state takes precedence)
        for (address, account) in plain_state.accounts {
            merged_state.accounts.insert(address, account);
        }

        for (address, storage) in plain_state.storages {
            let merged_storage =
                merged_state.storages.entry(address).or_insert_with(HashMap::default);
            merged_storage.extend(storage);
        }

        // Delegate to historical provider
        tracing::debug!(target: "provider", "MemoryOverlayStateProviderRef delegating state_root_with_updates_plain to historical provider");
        let result = self.historical.state_root_with_updates_plain(merged_state);
        if result.is_err() {
            tracing::error!(target: "provider", "MemoryOverlayStateProviderRef: historical provider failed state_root_with_updates_plain");
        }
        result
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
