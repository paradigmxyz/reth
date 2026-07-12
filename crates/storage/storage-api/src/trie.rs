use alloc::vec::Vec;
use alloy_primitives::{Address, Bytes, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;
use reth_trie_common::{
    updates::{StorageTrieUpdatesSorted, TrieUpdates, TrieUpdatesSorted},
    AccountProof, ExecutionWitnessMode, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};

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

/// A type that can iterate over consecutive hashed accounts and storage slots, and generate
/// boundary proofs for them, for serving `snap/2` (EIP-8189) `GetAccountRange`/`GetStorageRanges`
/// requests. Hash-native throughout, unlike [`StorageRootProvider`].
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StateRangeProvider {
    /// Returns accounts (hash, account) in `[start, limit]`, bounded by `response_bytes`.
    fn account_range(
        &self,
        start: B256,
        limit: B256,
        response_bytes: usize,
    ) -> RangeResult<(B256, Account)>;

    /// Returns the storage root for `hashed_address` without needing its address preimage.
    fn storage_root_by_hash(&self, hashed_address: B256) -> ProviderResult<B256>;

    /// Same as [`Self::account_range`], but for the storage slots of `hashed_address`.
    fn storage_range(
        &self,
        hashed_address: B256,
        start: B256,
        limit: B256,
        response_bytes: usize,
    ) -> RangeResult<(B256, U256)>;

    /// Returns an account-trie boundary proof for the already-hashed `keys`.
    fn account_range_proof(&self, keys: &[B256]) -> ProviderResult<Vec<Bytes>>;

    /// Same as [`Self::account_range_proof`], but for the storage trie of `hashed_address`.
    fn storage_range_proof(
        &self,
        hashed_address: B256,
        keys: &[B256],
    ) -> ProviderResult<Vec<Bytes>>;
}

/// A type that resolves retained state roots into reusable state range views.
#[auto_impl::auto_impl(&, Arc)]
pub trait StateRangeProviderFactory {
    /// Returns a view pinned to `state_root`, or `None` if that root is not retained.
    fn state_range_provider(&self, state_root: B256) -> ProviderResult<Option<StateRangeView>>;
}

/// A range query's items and whether the requested range is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeResponse<T> {
    /// The items found within the requested range, in ascending key order.
    pub items: Vec<T>,
    /// Whether `items` covers the entire requested range, or was cut short by the byte budget.
    pub complete: bool,
}

/// Result of a [`StateRangeProvider`] range query.
pub type RangeResult<T> = ProviderResult<RangeResponse<T>>;

/// A reusable state range view resolved for a specific state root.
pub type StateRangeView = Box<dyn StateRangeProvider + Send + 'static>;

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
