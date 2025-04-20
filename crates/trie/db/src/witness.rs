use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{map::B256Map, Bytes};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::TrieWitnessError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    witness::TrieWitness, HashedPostState, TrieInput,
};

/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness {
    /// Create a new [`TrieWitness`] from database transaction.
    fn from_tx(&self) -> Self;

    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<'a, TX: DbTx> DatabaseTrieWitness
    for TrieWitness<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(&self) -> Self {
        Self::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
    }

    fn overlay_witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(&self)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                self.trie_cursor_factory.clone(),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                self.hashed_cursor_factory.clone(),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .compute(target)
    }
}
