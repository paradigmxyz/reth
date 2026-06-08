use alloc::vec::Vec;
#[cfg(feature = "lattice-state-root")]
use alloy_primitives::U256;
use alloy_primitives::{Address, Bytes, B256};
use reth_storage_errors::provider::ProviderResult;
#[cfg(feature = "lattice-state-root")]
use reth_trie_common::lattice::LatticeAccumulatorUpdates;
use reth_trie_common::{
    updates::{StorageTrieUpdatesSorted, TrieUpdates, TrieUpdatesSorted},
    AccountProof, ExecutionWitnessMode, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
#[cfg(feature = "lattice-state-root")]
use revm_database::BundleState;

/// A type that can compute the state root of a given post state.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StateRootProvider {
    /// Returns the state root of the `BundleState` on top of the current state.
    ///
    /// # Note
    ///
    /// It is recommended to provide a different implementation from
    /// `state_root_with_updates` since it affects the memory usage during state root
    /// computation.
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256>;

    /// Returns the state root of the `HashedPostState` on top of the current state but reuses the
    /// intermediate nodes to speed up the computation. It's up to the caller to construct the
    /// prefix sets and inform the provider of the trie paths that have changes.
    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256>;

    /// Returns the state root of the `HashedPostState` on top of the current state with trie
    /// updates to be committed to the database.
    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)>;

    /// Returns a lattice state root and full lattice accumulator updates for the provided EVM
    /// bundle state.
    #[cfg(feature = "lattice-state-root")]
    fn lattice_state_root(
        &self,
        _bundle_state: &BundleState,
    ) -> ProviderResult<(B256, LatticeAccumulatorUpdates)> {
        Ok((B256::ZERO, Default::default()))
    }

    /// Returns the current flat lattice accumulator seed for incremental updates.
    #[cfg(feature = "lattice-state-root")]
    fn lattice_accumulator_seed(&self) -> ProviderResult<LatticeAccumulatorUpdates> {
        Ok(Default::default())
    }

    /// Returns nonzero hashed storage slots for `hashed_address`, used when flattening storage
    /// wipe deltas into the single lattice accumulator.
    #[cfg(feature = "lattice-state-root")]
    fn lattice_account_storage(&self, _hashed_address: B256) -> ProviderResult<Vec<(B256, U256)>> {
        Ok(Vec::new())
    }

    /// Returns state root and trie updates.
    /// See [`StateRootProvider::state_root_from_nodes`] for more info.
    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)>;
}

/// A type that can compute the storage root for a given account.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StorageRootProvider {
    /// Returns the storage root of the `HashedStorage` for target address on top of the current
    /// state.
    fn storage_root(&self, address: Address, hashed_storage: HashedStorage)
        -> ProviderResult<B256>;

    /// Returns the storage proof of the `HashedStorage` for target slot on top of the current
    /// state.
    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof>;

    /// Returns the storage multiproof for target slots.
    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof>;
}

/// A type that can generate state proof on top of a given post state.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StateProofProvider {
    /// Get account and storage proofs of target keys in the `HashedPostState`
    /// on top of the current state.
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof>;

    /// Generate [`MultiProof`] for target hashed account and corresponding
    /// hashed storage slot keys.
    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof>;

    /// Get trie witness for provided state using the given witness generation mode.
    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>>;
}

/// Trie Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait TrieWriter: Send {
    /// Writes trie updates to the database.
    ///
    /// Returns the number of entries modified.
    fn write_trie_updates(&self, trie_updates: TrieUpdates) -> ProviderResult<usize> {
        self.write_trie_updates_sorted(&trie_updates.into_sorted())
    }

    /// Writes trie updates to the database with already sorted updates.
    ///
    /// Returns the number of entries modified.
    fn write_trie_updates_sorted(&self, trie_updates: &TrieUpdatesSorted) -> ProviderResult<usize>;
}

/// Storage Trie Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StorageTrieWriter: Send {
    /// Writes storage trie updates from the given storage trie map with already sorted updates.
    ///
    /// Expects the storage trie updates to already be sorted by the hashed address key.
    ///
    /// Returns the number of entries modified.
    fn write_storage_trie_updates_sorted<'a>(
        &self,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdatesSorted)>,
    ) -> ProviderResult<usize>;
}
