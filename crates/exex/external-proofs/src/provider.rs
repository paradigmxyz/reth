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

use crate::storage::{ExternalHashedCursor, ExternalStorageError};
use crate::{
    proof::{
        DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot,
        DatabaseTrieWitness,
    },
    storage::ExternalStorage,
};

pub(crate) struct ExternalOverlayStateProviderRef<'a, P: ExternalStorage> {
    /// Historical state provider for non-state related tasks.
    latest: Box<dyn StateProvider + 'a>,

    /// Storage provider for state lookups.
    storage: P,

    /// Max block number that can be used for state lookups.
    block_number: BlockNumber,
}

impl<'a, P: ExternalStorage> ExternalOverlayStateProviderRef<'a, P> {
    pub(crate) fn new(
        latest: Box<dyn StateProvider + 'a>,
        storage: P,
        block_number: BlockNumber,
    ) -> Self {
        Self { latest, storage, block_number }
    }
}

impl From<ExternalStorageError> for ProviderError {
    fn from(error: ExternalStorageError) -> Self {
        Self::other(error)
    }
}

impl<'a, P: ExternalStorage> BlockHashReader for ExternalOverlayStateProviderRef<'a, P> {
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

impl<'a, P: ExternalStorage + Clone> StateRootProvider for ExternalOverlayStateProviderRef<'a, P> {
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

impl<'a, P: ExternalStorage + Clone> StorageRootProvider
    for ExternalOverlayStateProviderRef<'a, P>
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

impl<'a, P: ExternalStorage + Clone> StateProofProvider for ExternalOverlayStateProviderRef<'a, P> {
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

impl<'a, P: ExternalStorage> HashedPostStateProvider for ExternalOverlayStateProviderRef<'a, P> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<'a, P: ExternalStorage> AccountReader for ExternalOverlayStateProviderRef<'a, P> {
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

impl<'a, P: ExternalStorage + Clone> StateProvider for ExternalOverlayStateProviderRef<'a, P> {
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

impl<'a, P: ExternalStorage> BytecodeReader for ExternalOverlayStateProviderRef<'a, P> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.latest.bytecode_by_hash(code_hash)
    }
}
