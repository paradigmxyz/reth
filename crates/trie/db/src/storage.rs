use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{keccak256, map::hash_map, Address, BlockNumber, B256};
use reth_db_api::{
    cursor::DbCursorRO, models::BlockNumberAddress, tables, transaction::DbTx, DatabaseError,
};
use reth_execution_errors::StorageRootError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, HashedPostState, HashedStorage, StorageRoot,
};

#[cfg(feature = "metrics")]
use reth_trie::metrics::TrieRootMetrics;

/// Extends [`StorageRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStorageRoot {
    /// Create a new storage root calculator from database transaction and raw address.
    #[allow(clippy::wrong_self_convention)]
    fn from_tx(&self, address: Address) -> Self;

    /// Create a new storage root calculator from database transaction and hashed address.
    #[allow(clippy::wrong_self_convention)]
    fn from_tx_hashed(&self, hashed_address: B256) -> Self;

    /// Calculates the storage root for this [`HashedStorage`] and returns it.
    fn overlay_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError>;
}

/// Extends [`HashedStorage`] with operations specific for working with a database transaction.
pub trait DatabaseHashedStorage<TX>: Sized {
    /// Initializes [`HashedStorage`] from reverts. Iterates over storage reverts from the specified
    /// block up to the current tip and aggregates them into hashed storage in reverse.
    fn from_reverts(tx: &TX, address: Address, from: BlockNumber) -> Result<Self, DatabaseError>;
}

impl<'a, TX: DbTx> DatabaseStorageRoot
    for StorageRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(&self, address: Address) -> Self {
        Self::new(
            self.trie_cursor_factory.clone(),
            self.hashed_cursor_factory.clone(),
            address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn from_tx_hashed(&self, hashed_address: B256) -> Self {
        Self::new_hashed(
            self.trie_cursor_factory.clone(),
            self.hashed_cursor_factory.clone(),
            hashed_address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn overlay_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            self.trie_cursor_factory.clone(),
            HashedPostStateCursorFactory::new(self.hashed_cursor_factory.clone(), &state_sorted),
            address,
            prefix_set,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
        .root()
    }
}

impl<TX: DbTx> DatabaseHashedStorage<TX> for HashedStorage {
    fn from_reverts(tx: &TX, address: Address, from: BlockNumber) -> Result<Self, DatabaseError> {
        let mut storage = Self::new(false);
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSets>()?;
        for entry in storage_changesets_cursor.walk_range(BlockNumberAddress((from, address))..)? {
            let (BlockNumberAddress((_, storage_address)), storage_change) = entry?;
            if storage_address == address {
                let hashed_slot = keccak256(storage_change.key);
                if let hash_map::Entry::Vacant(entry) = storage.storage.entry(hashed_slot) {
                    entry.insert(storage_change.value);
                }
            }
        }
        Ok(storage)
    }
}
