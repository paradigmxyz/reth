//! Provider for external proofs storage

use crate::{
    proof::{
        DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot,
        DatabaseTrieWitness,
    },
    storage::{OpProofsHashedCursor, OpProofsStorage, OpProofsStorageError},
};
use alloy_primitives::keccak256;
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, ProviderError,
    ProviderResult, StateProofProvider, StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_revm::{
    db::BundleState,
    primitives::{alloy_primitives::BlockNumber, Address, Bytes, StorageValue, B256},
};
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, KeccakKeyHasher, MultiProof, MultiProofTargets,
    StateRoot, StorageMultiProof, StorageRoot, TrieInput,
};
use std::fmt::Debug;

/// State provider for external proofs storage.
pub struct OpProofsStateProviderRef<'a, Storage: OpProofsStorage> {
    /// Historical state provider for non-state related tasks.
    latest: Box<dyn StateProvider + 'a>,

    /// Storage provider for state lookups.
    storage: Storage,

    /// Max block number that can be used for state lookups.
    block_number: BlockNumber,
}

impl<'a, Storage: OpProofsStorage + Debug> Debug for OpProofsStateProviderRef<'a, Storage> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpProofsStateProviderRef")
            .field("storage", &self.storage)
            .field("block_number", &self.block_number)
            .finish()
    }
}

impl<'a, Storage: OpProofsStorage> OpProofsStateProviderRef<'a, Storage> {
    /// Creates a new `OpProofsStateProviderRef` instance.
    pub fn new(
        latest: Box<dyn StateProvider + 'a>,
        storage: Storage,
        block_number: BlockNumber,
    ) -> Self {
        Self { latest, storage, block_number }
    }
}

impl From<OpProofsStorageError> for ProviderError {
    fn from(error: OpProofsStorageError) -> Self {
        Self::other(error)
    }
}

impl<'a, Storage: OpProofsStorage> BlockHashReader for OpProofsStateProviderRef<'a, Storage> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.latest.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.latest.canonical_hashes_range(start, end)
    }
}

impl<'a, Storage: OpProofsStorage + Clone> StateRootProvider
    for OpProofsStateProviderRef<'a, Storage>
{
    fn state_root(&self, state: HashedPostState) -> ProviderResult<B256> {
        StateRoot::overlay_root(self.storage.clone(), self.block_number, state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        StateRoot::overlay_root_from_nodes(self.storage.clone(), self.block_number, input)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_with_updates(self.storage.clone(), self.block_number, state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_from_nodes_with_updates(
            self.storage.clone(),
            self.block_number,
            input,
        )
        .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<'a, Storage: OpProofsStorage + Clone> StorageRootProvider
    for OpProofsStateProviderRef<'a, Storage>
{
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        StorageRoot::overlay_root(self.storage.clone(), self.block_number, address, storage)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        StorageProof::overlay_storage_proof(
            self.storage.clone(),
            self.block_number,
            address,
            slot,
            storage,
        )
        .map_err(ProviderError::from)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        StorageProof::overlay_storage_multiproof(
            self.storage.clone(),
            self.block_number,
            address,
            slots,
            storage,
        )
        .map_err(ProviderError::from)
    }
}

impl<'a, Storage: OpProofsStorage + Clone> StateProofProvider
    for OpProofsStateProviderRef<'a, Storage>
{
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Proof::overlay_account_proof(self.storage.clone(), self.block_number, input, address, slots)
            .map_err(ProviderError::from)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        Proof::overlay_multiproof(self.storage.clone(), self.block_number, input, targets)
            .map_err(ProviderError::from)
    }

    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        TrieWitness::overlay_witness(self.storage.clone(), self.block_number, input, target)
            .map_err(ProviderError::from)
            .map(|hm| hm.into_values().collect())
    }
}

impl<'a, Storage: OpProofsStorage> HashedPostStateProvider
    for OpProofsStateProviderRef<'a, Storage>
{
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<'a, Storage: OpProofsStorage> AccountReader for OpProofsStateProviderRef<'a, Storage> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let hashed_key = keccak256(address.0);
        Ok(self
            .storage
            .account_hashed_cursor(self.block_number)
            .map_err(Into::<ProviderError>::into)?
            .seek(hashed_key)
            .map_err(Into::<ProviderError>::into)?
            .and_then(|(key, account)| (key == hashed_key).then_some(account)))
    }
}

impl<'a, Storage: OpProofsStorage + Clone> StateProvider for OpProofsStateProviderRef<'a, Storage> {
    fn storage(&self, address: Address, storage_key: B256) -> ProviderResult<Option<StorageValue>> {
        let hashed_key = keccak256(storage_key);
        Ok(self
            .storage
            .storage_hashed_cursor(keccak256(address.0), self.block_number)
            .map_err(Into::<ProviderError>::into)?
            .seek(hashed_key)
            .map_err(Into::<ProviderError>::into)?
            .and_then(|(key, storage)| (key == hashed_key).then_some(storage)))
    }
}

impl<'a, Storage: OpProofsStorage> BytecodeReader for OpProofsStateProviderRef<'a, Storage> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.latest.bytecode_by_hash(code_hash)
    }
}
