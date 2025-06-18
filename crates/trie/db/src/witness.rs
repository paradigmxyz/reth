use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{map::B256Map, Bytes};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::TrieWitnessError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    witness::TrieWitness, HashedPostState, TrieInput,
};

/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness<'a, TX> {
    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<'a, TX: DbTx> DatabaseTrieWitness<'a, TX>
    for TrieWitness<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn overlay_witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        let trie_cursor_factory = self.trie_cursor_factory().clone();
        let hashed_cursor_factory = self.hashed_cursor_factory().clone();
        Self::new(trie_cursor_factory, hashed_cursor_factory)
            .with_trie_cursor_factory(|factory| {
                InMemoryTrieCursorFactory::new(factory, &nodes_sorted)
            })
            .with_hashed_cursor_factory(|factory| {
                HashedPostStateCursorFactory::new(factory, &state_sorted)
            })
            .with_prefix_sets_mut(input.prefix_sets)
            .compute(target)
    }
}
