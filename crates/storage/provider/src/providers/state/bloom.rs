use crate::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{BytecodeReader, StateProofProvider, StorageRootProvider};
use reth_storage_bloom::StorageBloomFilter;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use std::sync::Arc;

/// A [`StateProvider`] wrapper that short-circuits storage reads using a bloom filter.
///
/// If the bloom filter reports that an `(address, slot)` pair is absent, the read
/// returns `Ok(None)` immediately without hitting the database. Otherwise it falls
/// through to the inner provider.
///
/// This follows the same wrapper pattern as `InstrumentedStateProvider` in
/// `crates/engine/tree/src/tree/instrumented_state.rs`.
#[derive(Debug)]
pub(crate) struct BloomStateProvider<S> {
    /// The wrapped state provider
    inner: S,
    /// Shared bloom filter for storage slot presence checks
    bloom: Arc<StorageBloomFilter>,
}

impl<S> BloomStateProvider<S> {
    /// Creates a new [`BloomStateProvider`] wrapping the given provider with a bloom filter.
    pub(crate) fn new(inner: S, bloom: Arc<StorageBloomFilter>) -> Self {
        Self { inner, bloom }
    }
}

impl<S: AccountReader> AccountReader for BloomStateProvider<S> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.inner.basic_account(address)
    }
}

impl<S: StateProvider> StateProvider for BloomStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        // Bloom filter check: if the filter says "not present", the slot is
        // guaranteed absent — skip the MDBX seek entirely.
        if !self.bloom.might_contain(&account, &storage_key) {
            return Ok(None);
        }
        self.inner.storage(account, storage_key)
    }

    fn storage_by_hashed_key(
        &self,
        address: Address,
        hashed_storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        // Cannot bloom-check pre-hashed keys — bloom stores unhashed (address, slot) pairs.
        self.inner.storage_by_hashed_key(address, hashed_storage_key)
    }
}

impl<S: BytecodeReader> BytecodeReader for BloomStateProvider<S> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.inner.bytecode_by_hash(code_hash)
    }
}

impl<S: StateRootProvider> StateRootProvider for BloomStateProvider<S> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.inner.state_root(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.inner.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.inner.state_root_from_nodes_with_updates(input)
    }
}

impl<S: StorageRootProvider> StorageRootProvider for BloomStateProvider<S> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.inner.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.inner.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.inner.storage_multiproof(address, slots, hashed_storage)
    }
}

impl<S: StateProofProvider> StateProofProvider for BloomStateProvider<S> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.inner.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.inner.multiproof(input, targets)
    }

    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        self.inner.witness(input, target)
    }
}

impl<S: BlockHashReader> BlockHashReader for BloomStateProvider<S> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.inner.canonical_hashes_range(start, end)
    }
}

impl<S: HashedPostStateProvider> HashedPostStateProvider for BloomStateProvider<S> {
    fn hashed_post_state(&self, bundle_state: &revm_database::BundleState) -> HashedPostState {
        self.inner.hashed_post_state(bundle_state)
    }
}
