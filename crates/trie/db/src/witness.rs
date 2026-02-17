use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{map::B256Map, Bytes};
use reth_db_api::{models::StorageLayout, transaction::DbTx};
use reth_execution_errors::TrieWitnessError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    witness::TrieWitness, HashedPostState, TrieInput,
};

/// Extends [`TrieWitness`] with operations specific for working with a database transaction.
pub trait DatabaseTrieWitness<'a, TX> {
    /// Create a new [`TrieWitness`] from database transaction.
    fn from_tx(tx: &'a TX, layout: StorageLayout) -> Self;

    /// Generates trie witness for target state based on [`TrieInput`].
    fn overlay_witness(
        tx: &'a TX,
        input: TrieInput,
        target: HashedPostState,
        layout: StorageLayout,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<'a, TX: DbTx> DatabaseTrieWitness<'a, TX>
    for TrieWitness<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX, layout: StorageLayout) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx, layout), DatabaseHashedCursorFactory::new(tx))
    }

    fn overlay_witness(
        tx: &'a TX,
        input: TrieInput,
        target: HashedPostState,
        layout: StorageLayout,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        TrieWitness::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx, layout),
                &nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .always_include_root_node()
        .compute(target)
    }
}
