use alloy_primitives::{
    keccak256,
    map::{B256Map, HashMap},
    Address, Bytes, B256, U256,
};
use reth_db_api::DatabaseError;
use reth_execution_errors::{StateProofError, StateRootError, StorageRootError, TrieWitnessError};
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::{
        HashedCursor, HashedCursorFactory, HashedPostStateCursorFactory, HashedStorageCursor,
    },
    metrics::TrieRootMetrics,
    proof::{Proof, StorageProof},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, BranchNodeCompact, HashedPostState, HashedPostStateSorted, HashedStorage,
    MultiProof, MultiProofTargets, Nibbles, StateRoot, StorageMultiProof, StorageRoot, TrieInput,
};

use crate::storage::{
    OpProofsHashedCursor, OpProofsStorage, OpProofsStorageError,
    OpProofsTrieCursor as OpProofsDBTrieCursor,
};

/// Manages reading storage or account trie nodes from external storage.
pub(crate) struct OpProofsTrieCursor<C>(pub(crate) C);

impl<C> OpProofsTrieCursor<C> {
    pub(crate) const fn new(preimage_cursor: C) -> Self {
        Self(preimage_cursor)
    }
}

impl From<OpProofsStorageError> for DatabaseError {
    fn from(error: OpProofsStorageError) -> Self {
        Self::Other(error.to_string())
    }
}

impl<C: OpProofsDBTrieCursor + Send + Sync> TrieCursor for OpProofsTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.0.seek_exact(key).map_err(Into::into)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.0.seek(key).map_err(Into::into)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.0.next().map_err(Into::into)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.0.current().map_err(Into::into)
    }
}

/// Factory for creating trie cursors for external storage.
#[derive(Clone)]
pub(crate) struct OpProofsTrieCursorFactory<P> {
    preimage_store: P,
    block_number: u64,
}

impl<P> OpProofsTrieCursorFactory<P> {
    pub(crate) const fn new(preimage_store: P, block_number: u64) -> Self {
        Self { preimage_store, block_number }
    }
}

impl<P: OpProofsStorage> TrieCursorFactory for OpProofsTrieCursorFactory<P> {
    type AccountTrieCursor = OpProofsTrieCursor<P::TrieCursor>;
    type StorageTrieCursor = OpProofsTrieCursor<P::TrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        Ok(OpProofsTrieCursor::new(
            self.preimage_store
                .trie_cursor(None, self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        Ok(OpProofsTrieCursor::new(
            self.preimage_store
                .trie_cursor(Some(hashed_address), self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }
}

/// Manages reading hashed account nodes from external storage.
#[derive(Clone)]
pub(crate) struct OpProofsHashedAccountCursor<C>(pub(crate) C);

impl<C> OpProofsHashedAccountCursor<C> {
    pub(crate) const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C: OpProofsHashedCursor<Value = Account> + Send + Sync> HashedCursor
    for OpProofsHashedAccountCursor<C>
{
    type Value = Account;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.seek(key).map_err(Into::into)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.next().map_err(Into::into)
    }
}

/// Manages reading hashed storage nodes from external storage.
#[derive(Clone)]
pub(crate) struct OpProofsHashedStorageCursor<C>(pub(crate) C);

impl<C> OpProofsHashedStorageCursor<C> {
    pub(crate) const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C: OpProofsHashedCursor<Value = U256> + Send + Sync> HashedCursor
    for OpProofsHashedStorageCursor<C>
{
    type Value = U256;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.seek(key).map_err(Into::into)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.next().map_err(Into::into)
    }
}

impl<C: OpProofsHashedCursor<Value = U256> + Send + Sync> HashedStorageCursor
    for OpProofsHashedStorageCursor<C>
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.0.is_storage_empty().map_err(Into::into)
    }
}

/// Factory for creating hashed account cursors for external storage.
#[derive(Clone)]
pub(crate) struct OpProofsHashedAccountCursorFactory<P> {
    preimage_store: P,
    block_number: u64,
}

impl<P> OpProofsHashedAccountCursorFactory<P> {
    pub(crate) const fn new(preimage_store: P, block_number: u64) -> Self {
        Self { preimage_store, block_number }
    }
}

impl<P: OpProofsStorage> HashedCursorFactory for OpProofsHashedAccountCursorFactory<P> {
    type AccountCursor = OpProofsHashedAccountCursor<P::AccountHashedCursor>;
    type StorageCursor = OpProofsHashedStorageCursor<P::StorageCursor>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(OpProofsHashedAccountCursor::new(
            self.preimage_store
                .account_hashed_cursor(self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(OpProofsHashedStorageCursor::new(
            self.preimage_store
                .storage_hashed_cursor(hashed_address, self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }
}

/// Extends [`Proof`] with operations specific for working with a database transaction.
pub(crate) trait DatabaseProof<P> {
    fn from_tx(preimage_store: P, block_number: u64) -> Self;

    /// Generates the state proof for target account based on [`TrieInput`].
    fn overlay_account_proof(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError>;

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError>;
}

impl<P: OpProofsStorage + Clone> DatabaseProof<P>
    for Proof<OpProofsTrieCursorFactory<P>, OpProofsHashedAccountCursorFactory<P>>
{
    /// Create a new [Proof] instance from database transaction.
    fn from_tx(preimage_store: P, block_number: u64) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
        )
    }

    fn overlay_account_proof(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(preimage_store.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .account_proof(address, slots)
    }

    fn overlay_multiproof(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(preimage_store.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .multiproof(targets)
    }
}

/// Extends [`StorageProof`] with operations specific for working with an external storage.
pub(crate) trait DatabaseStorageProof<P> {
    /// Create a new [`StorageProof`] from database transaction and account address.
    fn from_tx(preimage_store: P, block_number: u64, address: Address) -> Self;

    /// Generates the storage proof for target slot based on [`TrieInput`].
    fn overlay_storage_proof(
        preimage_store: P,
        block_number: u64,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInput`].
    fn overlay_storage_multiproof(
        preimage_store: P,
        block_number: u64,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError>;
}

impl<P: OpProofsStorage + Clone> DatabaseStorageProof<P>
    for StorageProof<OpProofsTrieCursorFactory<P>, OpProofsHashedAccountCursorFactory<P>>
{
    fn from_tx(preimage_store: P, block_number: u64, address: Address) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
            address,
        )
    }

    fn overlay_storage_proof(
        preimage_store: P,
        block_number: u64,
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
        Self::from_tx(preimage_store.clone(), block_number, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ))
            .with_prefix_set_mut(prefix_set)
            .storage_proof(slot)
    }

    fn overlay_storage_multiproof(
        preimage_store: P,
        block_number: u64,
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
        Self::from_tx(preimage_store.clone(), block_number, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ))
            .with_prefix_set_mut(prefix_set)
            .storage_multiproof(targets)
    }
}

pub(crate) trait DatabaseStateRoot<P>: Sized {
    /// Calculate the state root for this [`HashedPostState`].
    /// Internally, this method retrieves prefixsets and uses them
    /// to calculate incremental state root.
    ///
    /// # Returns
    ///
    /// The state root for this [`HashedPostState`].
    fn overlay_root(
        preimage_store: P,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root for this [`HashedPostState`] and returns it alongside trie
    /// updates. See [`Self::overlay_root`] for more info.
    fn overlay_root_with_updates(
        preimage_store: P,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostState`] using cached intermediate nodes.
    fn overlay_root_from_nodes(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostState`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_with_updates(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError>;
}

impl<P: OpProofsStorage + Clone> DatabaseStateRoot<P>
    for StateRoot<OpProofsTrieCursorFactory<P>, OpProofsHashedAccountCursorFactory<P>>
{
    fn overlay_root(
        preimage_store: P,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        preimage_store: P,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(prefix_sets)
        .root_with_updates()
    }

    fn overlay_root_from_nodes(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
    ) -> Result<B256, StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
                &nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()
    }

    fn overlay_root_from_nodes_with_updates(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
                &nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()
    }
}

pub(crate) trait DatabaseStorageRoot<P> {
    fn overlay_root(
        preimage_store: P,
        block_number: u64,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError>;
}

impl<P: OpProofsStorage + Clone> DatabaseStorageRoot<P>
    for StorageRoot<OpProofsTrieCursorFactory<P>, OpProofsHashedAccountCursorFactory<P>>
{
    fn overlay_root(
        preimage_store: P,
        block_number: u64,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ),
            address,
            prefix_set,
            // #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
        .root()
    }
}

pub(crate) trait DatabaseTrieWitness<P> {
    fn from_tx(preimage_store: P, block_number: u64) -> Self;

    fn overlay_witness(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<P: OpProofsStorage + Clone> DatabaseTrieWitness<P>
    for TrieWitness<OpProofsTrieCursorFactory<P>, OpProofsHashedAccountCursorFactory<P>>
{
    fn from_tx(preimage_store: P, block_number: u64) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
        )
    }

    fn overlay_witness(
        preimage_store: P,
        block_number: u64,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(preimage_store.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(preimage_store.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(preimage_store, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .always_include_root_node()
            .compute(target)
    }
}
