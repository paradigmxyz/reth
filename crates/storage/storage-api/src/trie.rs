use reth_primitives::{Address, Bytes, B256};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage};
use revm::db::BundleState;
use std::collections::HashMap;

/// A type that can compute the state root of a given post state.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StateRootProvider: Send + Sync {
    /// Returns the state root of the `BundleState` on top of the current state.
    ///
    /// # Note
    ///
    /// It is recommended to provide a different implementation from
    /// `state_root_with_updates` since it affects the memory usage during state root
    /// computation.
    fn state_root(&self, bundle_state: &BundleState) -> ProviderResult<B256> {
        self.hashed_state_root(HashedPostState::from_bundle_state(&bundle_state.state))
    }

    /// Returns the state root of the `HashedPostState` on top of the current state.
    fn hashed_state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256>;

    /// Returns the state root of the BundleState on top of the current state with trie
    /// updates to be committed to the database.
    fn state_root_with_updates(
        &self,
        bundle_state: &BundleState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.hashed_state_root_with_updates(HashedPostState::from_bundle_state(&bundle_state.state))
    }

    /// Returns the state root of the `HashedPostState` on top of the current state with trie
    /// updates to be committed to the database.
    fn hashed_state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)>;

    /// Returns the storage root of the `HashedStorage` for target address on top of the current
    /// state.
    fn hashed_storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256>;
}

/// A type that can generate state proof on top of a given post state.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait StateProofProvider: Send + Sync {
    /// Get account and storage proofs of target keys in the `BundleState`
    /// on top of the current state.
    fn proof(
        &self,
        state: &BundleState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let hashed_state = HashedPostState::from_bundle_state(&state.state);
        self.hashed_proof(hashed_state, address, slots)
    }

    /// Get account and storage proofs of target keys in the `HashedPostState`
    /// on top of the current state.
    fn hashed_proof(
        &self,
        hashed_state: HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof>;

    /// Get trie witness for provided state.
    fn witness(
        &self,
        overlay: HashedPostState,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>>;
}
