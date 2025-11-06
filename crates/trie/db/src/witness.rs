use alloy_primitives::{map::B256Map, Bytes};
use reth_execution_errors::TrieWitnessError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    witness::TrieWitness,
    HashedPostState, TrieInput,
};

/// Extends [`TrieWitness`] with operations specific for working with a database provider.
pub trait DatabaseTrieWitness<'a, P> {
    /// The provider type.
    type Provider;

    /// Create a new [`TrieWitness`] from database provider.
    fn from_provider(provider: &'a Self::Provider) -> Self;

    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        provider: &'a P,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<'a, P> DatabaseTrieWitness<'a, P> for TrieWitness<&'a P, &'a P>
where
    P: TrieCursorFactory + HashedCursorFactory + Sync,
{
    type Provider = P;

    fn from_provider(provider: &'a Self::Provider) -> Self {
        Self::new(provider, provider)
    }

    fn overlay_witness(
        provider: &'a P,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_provider(provider)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(provider, &nodes_sorted))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(provider, &state_sorted))
            .with_prefix_sets_mut(input.prefix_sets)
            .always_include_root_node()
            .compute(target)
    }
}
