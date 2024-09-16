use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{Address, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StateProofError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::Proof,
    trie_cursor::InMemoryTrieCursorFactory, MultiProof, TrieInput,
};
use reth_trie_common::AccountProof;
use std::collections::{HashMap, HashSet};

/// Extends [`Proof`] with operations specific for working with a database transaction.
pub trait DatabaseProof<'a, TX> {
    /// Create a new [Proof] from database transaction.
    fn from_tx(tx: &'a TX) -> Self;

    /// Generates the state proof for target account based on [`TrieInput`].
    fn overlay_account_proof(
        tx: &'a TX,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError>;

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        tx: &'a TX,
        input: TrieInput,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> Result<MultiProof, StateProofError>;
}

impl<'a, TX: DbTx> DatabaseProof<'a, TX>
    for Proof<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    /// Create a new [Proof] instance from database transaction.
    fn from_tx(tx: &'a TX) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx), DatabaseHashedCursorFactory::new(tx))
    }

    fn overlay_account_proof(
        tx: &'a TX,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
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
            .account_proof(address, slots)
    }

    fn overlay_multiproof(
        tx: &'a TX,
        input: TrieInput,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> Result<MultiProof, StateProofError> {
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
            .with_targets(targets)
            .multiproof()
    }
}
