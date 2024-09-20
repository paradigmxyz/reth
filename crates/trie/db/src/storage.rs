use std::collections::hash_map;

use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{keccak256, Address, BlockNumber, B256};
use reth_db::{cursor::DbCursorRO, models::BlockNumberAddress, tables, DatabaseError};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StorageRootError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, HashedPostState, HashedStorage, StorageRoot,
};

#[cfg(feature = "metrics")]
use reth_trie::metrics::{TrieRootMetrics, TrieType};

/// Extends [`StorageRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStorageRoot<'a, TX> {
    /// Create a new storage root calculator from database transaction and raw address.
    fn from_tx(tx: &'a TX, address: Address) -> Self;

    /// Create a new storage root calculator from database transaction and hashed address.
    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self;

    /// Calculates the storage root for this [`HashedStorage`] and returns it.
    fn overlay_root(
        tx: &'a TX,
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

impl<'a, TX: DbTx> DatabaseStorageRoot<'a, TX>
    for StorageRoot<DatabaseTrieCursorFactory<'a, TX>, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }

    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self {
        Self::new_hashed(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            hashed_address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }

    fn overlay_root(
        tx: &'a TX,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
            address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
        .with_prefix_set(prefix_set)
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
