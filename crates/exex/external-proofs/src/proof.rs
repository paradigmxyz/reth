//! Provides proof operation implementations for external proof storage.

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
#[derive(Debug, Clone)]
pub struct OpProofsTrieCursor<C: OpProofsDBTrieCursor>(pub C);

impl<C: OpProofsDBTrieCursor> OpProofsTrieCursor<C> {
    /// Creates a new `OpProofsTrieCursor` instance.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
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
#[derive(Debug, Clone)]
pub struct OpProofsTrieCursorFactory<Storage: OpProofsStorage> {
    storage: Storage,
    block_number: u64,
}

impl<Storage: OpProofsStorage> OpProofsTrieCursorFactory<Storage> {
    /// Creates a new `OpProofsTrieCursorFactory` instance.
    pub const fn new(storage: Storage, block_number: u64) -> Self {
        Self { storage, block_number }
    }
}

impl<Storage: OpProofsStorage> TrieCursorFactory for OpProofsTrieCursorFactory<Storage> {
    type AccountTrieCursor = OpProofsTrieCursor<Storage::AccountTrieCursor>;
    type StorageTrieCursor = OpProofsTrieCursor<Storage::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        Ok(OpProofsTrieCursor::new(
            self.storage
                .account_trie_cursor(self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        Ok(OpProofsTrieCursor::new(
            self.storage
                .storage_trie_cursor(hashed_address, self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }
}

/// Manages reading hashed account nodes from external storage.
#[derive(Debug, Clone)]
pub struct OpProofsHashedAccountCursor<C>(pub C);

impl<C> OpProofsHashedAccountCursor<C> {
    /// Creates a new `OpProofsHashedAccountCursor` instance.
    pub const fn new(cursor: C) -> Self {
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
#[derive(Debug, Clone)]
pub struct OpProofsHashedStorageCursor<C: OpProofsHashedCursor<Value = U256>>(pub C);

impl<C: OpProofsHashedCursor<Value = U256>> OpProofsHashedStorageCursor<C> {
    /// Creates a new `OpProofsHashedStorageCursor` instance.
    pub const fn new(cursor: C) -> Self {
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
#[derive(Debug, Clone)]
pub struct OpProofsHashedAccountCursorFactory<Storage: OpProofsStorage> {
    storage: Storage,
    block_number: u64,
}

impl<Storage: OpProofsStorage> OpProofsHashedAccountCursorFactory<Storage> {
    /// Creates a new `OpProofsHashedAccountCursorFactory` instance.
    pub const fn new(storage: Storage, block_number: u64) -> Self {
        Self { storage, block_number }
    }
}

impl<Storage: OpProofsStorage> HashedCursorFactory for OpProofsHashedAccountCursorFactory<Storage> {
    type AccountCursor = OpProofsHashedAccountCursor<Storage::AccountHashedCursor>;
    type StorageCursor = OpProofsHashedStorageCursor<Storage::StorageCursor>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(OpProofsHashedAccountCursor::new(
            self.storage
                .account_hashed_cursor(self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(OpProofsHashedStorageCursor::new(
            self.storage
                .storage_hashed_cursor(hashed_address, self.block_number)
                .map_err(Into::<DatabaseError>::into)?,
        ))
    }
}

/// Extends [`Proof`] with operations specific for working with external storage.
pub trait DatabaseProof<Storage> {
    /// Creates a new `DatabaseProof` instance from external storage.
    fn from_tx(storage: Storage, block_number: u64) -> Self;

    /// Generates the state proof for target account based on [`TrieInput`].
    fn overlay_account_proof(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError>;

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError>;
}

impl<Storage: OpProofsStorage + Clone> DatabaseProof<Storage>
    for Proof<OpProofsTrieCursorFactory<Storage>, OpProofsHashedAccountCursorFactory<Storage>>
{
    /// Create a new [Proof] instance from external storage.
    fn from_tx(storage: Storage, block_number: u64) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(storage, block_number),
        )
    }

    /// Generates the state proof for target account based on [`TrieInput`].
    fn overlay_account_proof(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(storage.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(storage.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .account_proof(address, slots)
    }

    /// Generates the state [`MultiProof`] for target hashed account and storage keys.
    fn overlay_multiproof(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(storage.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(storage.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .multiproof(targets)
    }
}

/// Extends [`StorageProof`] with operations specific for working with an external storage.
pub trait DatabaseStorageProof<Storage> {
    /// Create a new [`StorageProof`] from external storage and account address.
    fn from_tx(storage: Storage, block_number: u64, address: Address) -> Self;

    /// Generates the storage proof for target slot based on [`TrieInput`].
    fn overlay_storage_proof(
        storage: Storage,
        block_number: u64,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError>;

    /// Generates the storage multiproof for target slots based on [`TrieInput`].
    fn overlay_storage_multiproof(
        storage: Storage,
        block_number: u64,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError>;
}

impl<Storage: OpProofsStorage + Clone> DatabaseStorageProof<Storage>
    for StorageProof<
        OpProofsTrieCursorFactory<Storage>,
        OpProofsHashedAccountCursorFactory<Storage>,
    >
{
    /// Create a new [`StorageProof`] from external storage and account address.
    fn from_tx(storage: Storage, block_number: u64, address: Address) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(storage, block_number),
            address,
        )
    }

    fn overlay_storage_proof(
        storage: Storage,
        block_number: u64,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> Result<reth_trie::StorageProof, StateProofError> {
        let hashed_address = keccak256(address);
        let prefix_set = hashed_storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, hashed_storage.into_sorted())]),
        );
        Self::from_tx(storage.clone(), block_number, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ))
            .with_prefix_set_mut(prefix_set)
            .storage_proof(slot)
    }

    fn overlay_storage_multiproof(
        storage: Storage,
        block_number: u64,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> Result<StorageMultiProof, StateProofError> {
        let hashed_address = keccak256(address);
        let targets = slots.iter().map(keccak256).collect();
        let prefix_set = hashed_storage.construct_prefix_set();
        let state_sorted = HashedPostStateSorted::new(
            Default::default(),
            HashMap::from_iter([(hashed_address, hashed_storage.into_sorted())]),
        );
        Self::from_tx(storage.clone(), block_number, address)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ))
            .with_prefix_set_mut(prefix_set)
            .storage_multiproof(targets)
    }
}

/// Extends [`StateRoot`] with operations specific for working with an external storage.
pub trait DatabaseStateRoot<Storage: OpProofsStorage + Clone>: Sized {
    /// Calculate the state root for this [`HashedPostState`].
    /// Internally, this method retrieves prefixsets and uses them
    /// to calculate incremental state root.
    ///
    /// # Returns
    ///
    /// The state root for this [`HashedPostState`].
    fn overlay_root(
        storage: Storage,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root for this [`HashedPostState`] and returns it alongside trie
    /// updates. See [`Self::overlay_root`] for more info.
    fn overlay_root_with_updates(
        storage: Storage,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostState`] using cached intermediate nodes.
    fn overlay_root_from_nodes(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostState`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_with_updates(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError>;
}

impl<Storage: OpProofsStorage + Clone> DatabaseStateRoot<Storage>
    for StateRoot<OpProofsTrieCursorFactory<Storage>, OpProofsHashedAccountCursorFactory<Storage>>
{
    fn overlay_root(
        storage: Storage,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        storage: Storage,
        block_number: u64,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(prefix_sets)
        .root_with_updates()
    }

    fn overlay_root_from_nodes(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
    ) -> Result<B256, StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(storage.clone(), block_number),
                &nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()
    }

    fn overlay_root_from_nodes_with_updates(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(storage.clone(), block_number),
                &nodes_sorted,
            ),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()
    }
}

/// Extends [`StorageRoot`] with operations specific for working with an external storage.
pub trait DatabaseStorageRoot<Storage: OpProofsStorage + Clone> {
    /// Calculates the storage root for provided [`HashedStorage`].
    fn overlay_root(
        storage: Storage,
        block_number: u64,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError>;
}

impl<Storage: OpProofsStorage + Clone> DatabaseStorageRoot<Storage>
    for StorageRoot<OpProofsTrieCursorFactory<Storage>, OpProofsHashedAccountCursorFactory<Storage>>
{
    fn overlay_root(
        storage: Storage,
        block_number: u64,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
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

/// Extends [`TrieWitness`] with operations specific for working with an external storage.
pub trait DatabaseTrieWitness<Storage: OpProofsStorage + Clone> {
    /// Creates a new [`TrieWitness`] instance from external storage.
    fn from_tx(storage: Storage, block_number: u64) -> Self;

    /// Generates the trie witness for the target state based on [`TrieInput`].
    fn overlay_witness(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError>;
}

impl<Storage: OpProofsStorage + Clone> DatabaseTrieWitness<Storage>
    for TrieWitness<OpProofsTrieCursorFactory<Storage>, OpProofsHashedAccountCursorFactory<Storage>>
{
    fn from_tx(storage: Storage, block_number: u64) -> Self {
        Self::new(
            OpProofsTrieCursorFactory::new(storage.clone(), block_number),
            OpProofsHashedAccountCursorFactory::new(storage, block_number),
        )
    }

    fn overlay_witness(
        storage: Storage,
        block_number: u64,
        input: TrieInput,
        target: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let nodes_sorted = input.nodes.into_sorted();
        let state_sorted = input.state.into_sorted();
        Self::from_tx(storage.clone(), block_number)
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                OpProofsTrieCursorFactory::new(storage.clone(), block_number),
                &nodes_sorted,
            ))
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                OpProofsHashedAccountCursorFactory::new(storage, block_number),
                &state_sorted,
            ))
            .with_prefix_sets_mut(input.prefix_sets)
            .always_include_root_node()
            .compute(target)
    }
}
