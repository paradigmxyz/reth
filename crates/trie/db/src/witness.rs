use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::TrieWitnessError;
use reth_primitives::{Bytes, B256};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, witness::TrieWitness, HashedPostState,
};
use std::collections::HashMap;

/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness<'a, TX> {
    /// Create a new [`TrieWitness`] from database transaction.
    fn from_tx(tx: &'a TX) -> Self;

    /// Generates trie witness for target state on top of this [`HashedPostState`].
    fn overlay_witness(
        tx: &'a TX,
        post_state: HashedPostState,
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
        post_state: HashedPostState,
        target: HashedPostState,
    ) -> Result<HashMap<B256, Bytes>, TrieWitnessError> {
        let prefix_sets = post_state.construct_prefix_sets();
        let sorted = post_state.into_sorted();
        let hashed_cursor_factory =
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &sorted);
        Self::from_tx(tx)
            .with_hashed_cursor_factory(hashed_cursor_factory)
            .with_prefix_sets_mut(prefix_sets)
            .compute(target)
    }
}
