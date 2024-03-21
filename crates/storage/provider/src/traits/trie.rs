use auto_impl::auto_impl;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::B256;
use reth_trie::updates::TrieUpdates;
use revm::db::BundleState;

/// A type that can compute the state root of a given post state.
#[auto_impl(&, Box, Arc)]
pub trait StateRootProvider: Send + Sync {
    /// Returns the state root of the `BundleState` on top of the current state.
    ///
    /// # Note
    ///
    /// It is recommended to provide a different implementation from
    /// `state_root_with_updates` since it affects the memory usage during state root
    /// computation.
    fn state_root(&self, bundle_state: &BundleState) -> ProviderResult<B256>;

    /// Returns the state root of the BundleState on top of the current state with trie
    /// updates to be committed to the database.
    fn state_root_with_updates(
        &self,
        bundle_state: &BundleState,
    ) -> ProviderResult<(B256, TrieUpdates)>;
}
