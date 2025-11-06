use alloy_primitives::{keccak256, map::HashMap, Address, B256};
use reth_execution_errors::StateProofError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    proof::{Proof, StorageProof},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    AccountProof, HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets,
    StorageMultiProof, TrieInput,
};

/// Extends [`Proof`] with operations specific for working with a database provider.
pub trait DatabaseProof<'a> {
    /// Associated type for the database provider.
    type Provider;

    /// Create a new [`Proof`] instance from database provider.
    fn from_provider(provider: &'a Self::Provider) -> Self;

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

impl<'a, P> DatabaseProof<'a> for Proof<&'a P, &'a P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    type Provider = P;

    fn from_provider(provider: &'a Self::Provider) -> Self {
        Self::new(provider, provider)
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
            InMemoryTrieCursorFactory::new(*self.trie_cursor_factory(), &nodes_sorted),
            HashedPostStateCursorFactory::new(*self.hashed_cursor_factory(), &state_sorted),
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
            InMemoryTrieCursorFactory::new(*self.trie_cursor_factory(), &nodes_sorted),
            HashedPostStateCursorFactory::new(*self.hashed_cursor_factory(), &state_sorted),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .multiproof(targets)
    }
}

/// Extends [`StorageProof`] with operations specific for working with a database provider.
pub trait DatabaseStorageProof<'a, P> {
    /// Create a new [`StorageProof`] from database provider and account address.
    fn from_provider(provider: &'a P, address: Address) -> Self;

    /// Generates the storage proof for target slot based on [`TrieInput`].
    fn overlay_storage_proof(
        provider: &'a P,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInput`].
    fn overlay_storage_multiproof(
        provider: &'a P,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError>;
}

impl<'a, P> DatabaseStorageProof<'a, P> for StorageProof<&'a P, &'a P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    fn from_provider(provider: &'a P, address: Address) -> Self {
        Self::new(provider, provider, address)
    }

    fn overlay_storage_proof(
        provider: &'a P,
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
        Self::from_provider(provider, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(provider, &state_sorted))
            .with_prefix_set_mut(prefix_set)
            .storage_proof(slot)
    }

    fn overlay_storage_multiproof(
        provider: &'a P,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError> {
        let hashed_address = keccak256(address);
        let targets = slots.iter().map(keccak256).collect();
        let prefix_set = storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, storage.into_sorted())]),
        );
        Self::from_provider(provider, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(provider, &state_sorted))
            .with_prefix_set_mut(prefix_set)
            .storage_multiproof(targets)
    }
}
