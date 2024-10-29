use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{map::HashMap, Bytes, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::TrieWitnessError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    witness::TrieWitness, HashedPostState, TrieInput,
};

/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness<'a, TX> {
    /// Create a new [`TrieWitness`] from database transaction.
    fn from_tx(tx: &'a TX) -> Self;

    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        tx: &'a TX,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<HashMap<B256, Bytes>, TrieWitnessError>;
}

impl<'a, TX: DbTx> DatabaseTrieWitness<'a, TX>
    for TrieWitness<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(tx: &'a TX) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx), DatabaseHashedCursorFactory::new(tx))
    }

    fn overlay_witness(
        tx: &'a TX,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<HashMap<B256, Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(tx)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(tx),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .compute(target)
    }
}
