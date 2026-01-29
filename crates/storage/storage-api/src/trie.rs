use alloc::vec::Vec;
use alloy_primitives::{map::HashMap, Address, Bytes, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie_common::{
    updates::{StorageTrieUpdatesSorted, TrieUpdates, TrieUpdatesSorted},
    AccountProof, HashedPostState, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
    StorageProof, TrieInput,
};

/// Plain (unhashed) post state used for TrieDB state root computation.
///
/// Unlike [`HashedPostState`] which uses hashed addresses as keys, this structure
/// uses raw addresses, allowing TrieDB to handle the hashing internally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlainPostState {
    /// Mapping of address to account info, `None` if destroyed.
    pub accounts: HashMap<Address, Option<Account>>,
    /// Mapping of address to storage entries (slot -> value).
    pub storages: HashMap<Address, HashMap<B256, U256>>,
}

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

    /// Returns state root and trie updates.
    /// See [`StateRootProvider::state_root_from_nodes`] for more info.
    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)>;

    /// Returns the state root of the `PlainPostState` on top of the current state with trie
    /// updates to be committed to the database.
    ///
    /// This method uses TrieDB for state root computation when available. Unlike other methods
    /// which take `HashedPostState` with pre-hashed addresses, this takes `PlainPostState` with
    /// unhashed addresses, allowing TrieDB to handle the hashing internally.
    ///
    /// # Errors
    ///
    /// Returns an error if TrieDB is not available for this provider.
    fn state_root_with_updates_plain(
        &self,
        plain_state: PlainPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let _ = plain_state;
        tracing::error!(target: "provider", provider_type = std::any::type_name::<Self>(), "state_root_with_updates_plain called on provider without triedb support");
        Err(ProviderError::UnsupportedProvider)
    }
}

/// A type that can compute the storage root for a given account.
#[auto_impl::auto_impl(&, Box)]
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
#[auto_impl::auto_impl(&, Box)]
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

    /// Get trie witness for provided state.
    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>>;
}

/// Trie Writer
#[auto_impl::auto_impl(&, Box)]
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
#[auto_impl::auto_impl(&, Box)]
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
