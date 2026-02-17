use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{keccak256, map::HashMap, Address, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StateProofError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    proof::{Proof, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    AccountProof, HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets,
    StorageMultiProof, TrieInput,
};

/// Extends [`Proof`] with operations specific for working with a database transaction.
pub trait DatabaseProof<'a> {
    /// Associated type for the database transaction.
    type Tx;

    /// Create a new [`Proof`] instance from database transaction.
    fn from_tx(tx: &'a Self::Tx, is_v2: bool) -> Self;

    /// Generates the state proof for target account based on [`TrieInput`].
    fn overlay_account_proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError>;

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError>;
}

impl<'a, TX: DbTx> DatabaseProof<'a>
    for Proof<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>
{
    type Tx = TX;

    fn from_tx(tx: &'a Self::Tx, is_v2: bool) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx, is_v2), DatabaseHashedCursorFactory::new(tx))
    }
    fn overlay_account_proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Proof::new(
            InMemoryTrieCursorFactory::new(self.trie_cursor_factory().clone(), &nodes_sorted),
            HashedPostStateCursorFactory::new(self.hashed_cursor_factory().clone(), &state_sorted),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .account_proof(address, slots)
    }

    fn overlay_multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Proof::new(
            InMemoryTrieCursorFactory::new(self.trie_cursor_factory().clone(), &nodes_sorted),
            HashedPostStateCursorFactory::new(self.hashed_cursor_factory().clone(), &state_sorted),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .multiproof(targets)
    }
}

/// Extends [`StorageProof`] with operations specific for working with a database transaction.
pub trait DatabaseStorageProof<'a, TX> {
    /// Create a new [`StorageProof`] from database transaction and account address.
    fn from_tx(tx: &'a TX, address: Address, is_v2: bool) -> Self;

    /// Generates the storage proof for target slot based on [`TrieInput`].
    fn overlay_storage_proof(
        tx: &'a TX,
        address: Address,
        slot: B256,
        storage: HashedStorage,
        is_v2: bool,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInput`].
    fn overlay_storage_multiproof(
        tx: &'a TX,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
        is_v2: bool,
    ) -> Result<StorageMultiProof, StateProofError>;
}

impl<'a, TX: DbTx> DatabaseStorageProof<'a, TX>
    for StorageProof<
        'static,
        DatabaseTrieCursorFactory<&'a TX>,
        DatabaseHashedCursorFactory<&'a TX>,
    >
{
    fn from_tx(tx: &'a TX, address: Address, is_v2: bool) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::new(tx, is_v2),
            DatabaseHashedCursorFactory::new(tx),
            address,
        )
    }

    fn overlay_storage_proof(
        tx: &'a TX,
        address: Address,
        slot: B256,
        storage: HashedStorage,
        is_v2: bool,
    ) -> Result<reth_trie::StorageProof, StateProofError> {
        let hashed_address = keccak256(address);
        let prefix_set = storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, storage.into_sorted())]),
        );
        StorageProof::new(
            DatabaseTrieCursorFactory::new(tx, is_v2),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
            address,
        )
        .with_prefix_set_mut(prefix_set)
        .storage_proof(slot)
    }

    fn overlay_storage_multiproof(
        tx: &'a TX,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
        is_v2: bool,
    ) -> Result<StorageMultiProof, StateProofError> {
        let hashed_address = keccak256(address);
        let targets = slots.iter().map(keccak256).collect();
        let prefix_set = storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, storage.into_sorted())]),
        );
        StorageProof::new(
            DatabaseTrieCursorFactory::new(tx, is_v2),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
            address,
        )
        .with_prefix_set_mut(prefix_set)
        .storage_multiproof(targets)
    }
}
