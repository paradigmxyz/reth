use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, TrieTableAdapter};
use alloy_primitives::{keccak256, map::HashMap, Address, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StateProofError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    proof::{Proof, StorageProof},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdatesSorted,
    AccountProof, HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets,
    StorageMultiProof, TrieInputSorted,
};

/// Extends [`Proof`] with operations specific for working with a database transaction.
pub trait DatabaseProof<'a> {
    /// Associated type for the database transaction.
    type Tx;

    /// Create a new [`Proof`] instance from database transaction.
    fn from_tx(tx: &'a Self::Tx) -> Self;

    /// Generates the state proof for target account based on [`TrieInputSorted`].
    fn overlay_account_proof(
        &self,
        input: TrieInputSorted,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError>;

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        &self,
        input: TrieInputSorted,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError>;
}

impl<'a, TX: DbTx, A: TrieTableAdapter> DatabaseProof<'a>
    for Proof<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>
{
    type Tx = TX;

    fn from_tx(tx: &'a Self::Tx) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx), DatabaseHashedCursorFactory::new(tx))
    }
    fn overlay_account_proof(
        &self,
        input: TrieInputSorted,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        with_overlay(self, input, |proof| proof.account_proof(address, slots))
    }

    fn overlay_multiproof(
        &self,
        input: TrieInputSorted,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError> {
        with_overlay(self, input, |proof| proof.multiproof(targets))
    }
}

/// Extends [`StorageProof`] with operations specific for working with a database transaction.
pub trait DatabaseStorageProof<'a, TX> {
    /// Create a new [`StorageProof`] from database transaction and account address.
    fn from_tx(tx: &'a TX, address: Address) -> Self;

    /// Generates the storage proof for target slot based on [`HashedStorage`].
    fn overlay_storage_proof(
        tx: &'a TX,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInputSorted`].
    fn overlay_storage_multiproof(
        tx: &'a TX,
        address: Address,
        slots: &[B256],
        input: TrieInputSorted,
    ) -> Result<StorageMultiProof, StateProofError>;
}

impl<'a, TX: DbTx, A: TrieTableAdapter> DatabaseStorageProof<'a, TX>
    for StorageProof<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::<_, A>::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            address,
        )
    }

    fn overlay_storage_proof(
        tx: &'a TX,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError> {
        let hashed_address = keccak256(address);
        let prefix_set = storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, storage.into_sorted())]),
        );
        StorageProof::new(
            DatabaseTrieCursorFactory::<_, A>::new(tx),
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
        mut input: TrieInputSorted,
    ) -> Result<StorageMultiProof, StateProofError> {
        let hashed_address = keccak256(address);
        let targets = slots.iter().map(keccak256).collect();
        StorageProof::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::<_, A>::new(tx),
                &input.nodes,
            ),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &input.state),
            address,
        )
        .with_prefix_set_mut(
            input.prefix_sets.storage_prefix_sets.remove(&hashed_address).unwrap_or_default(),
        )
        .storage_multiproof(targets)
    }
}

// Keeps cursor layering and prefix invalidation identical across proof entrypoints.
fn with_overlay<T: Clone, H: Clone, R>(
    proof: &Proof<T, H>,
    input: TrieInputSorted,
    f: impl FnOnce(
        Proof<
            InMemoryTrieCursorFactory<T, &TrieUpdatesSorted>,
            HashedPostStateCursorFactory<H, &HashedPostStateSorted>,
        >,
    ) -> R,
) -> R {
    f(Proof::new(
        InMemoryTrieCursorFactory::new(proof.trie_cursor_factory().clone(), input.nodes.as_ref()),
        HashedPostStateCursorFactory::new(
            proof.hashed_cursor_factory().clone(),
            input.state.as_ref(),
        ),
    )
    .with_prefix_sets_mut(input.prefix_sets))
}
