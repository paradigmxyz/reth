use alloc::vec::Vec;
use alloy_primitives::{map::HashMap, Address, BlockNumber, Bytes, B256, U256};
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

/// Trie Reader
#[auto_impl::auto_impl(&, Box)]
pub trait TrieReader: Send {
    /// Returns the [`TrieUpdatesSorted`] for reverting the trie database to its state prior to the
    /// given block and onwards having been processed.
    fn trie_reverts(&self, from: BlockNumber) -> ProviderResult<TrieUpdatesSorted>;

    /// Returns the trie updates that were applied by the specified block.
    fn get_block_trie_updates(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<TrieUpdatesSorted>;
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

    /// Records the current values of all trie nodes which will be updated using the [`TrieUpdates`]
    /// into the trie changesets tables.
    ///
    /// The intended usage of this method is to call it _prior_ to calling `write_trie_updates` with
    /// the same [`TrieUpdates`].
    ///
    /// The `updates_overlay` parameter allows providing additional in-memory trie updates that
    /// should be considered when looking up current node values. When provided, these overlay
    /// updates are applied on top of the database state, allowing the method to see a view that
    /// includes both committed database values and pending in-memory changes. This is useful
    /// when writing changesets for updates that depend on previous uncommitted trie changes.
    ///
    /// Returns the number of keys written.
    fn write_trie_changesets(
        &self,
        block_number: BlockNumber,
        trie_updates: &TrieUpdatesSorted,
        updates_overlay: Option<&TrieUpdatesSorted>,
    ) -> ProviderResult<usize>;

    /// Clears contents of trie changesets completely
    fn clear_trie_changesets(&self) -> ProviderResult<()>;

    /// Clears contents of trie changesets starting from the given block number (inclusive) onwards.
    fn clear_trie_changesets_from(&self, from: BlockNumber) -> ProviderResult<()>;
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

    /// Records the current values of all trie nodes which will be updated using the
    /// [`StorageTrieUpdatesSorted`] into the storage trie changesets table.
    ///
    /// The intended usage of this method is to call it _prior_ to calling
    /// `write_storage_trie_updates` with the same set of [`StorageTrieUpdatesSorted`].
    ///
    /// The `updates_overlay` parameter allows providing additional in-memory trie updates that
    /// should be considered when looking up current node values. When provided, these overlay
    /// updates are applied on top of the database state for each storage trie, allowing the
    /// method to see a view that includes both committed database values and pending in-memory
    /// changes. This is useful when writing changesets for storage updates that depend on
    /// previous uncommitted trie changes.
    ///
    /// Returns the number of keys written.
    fn write_storage_trie_changesets<'a>(
        &self,
        block_number: BlockNumber,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdatesSorted)>,
        updates_overlay: Option<&TrieUpdatesSorted>,
    ) -> ProviderResult<usize>;
}
