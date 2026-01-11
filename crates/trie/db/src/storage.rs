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

/// A type alias for [`StorageRoot`] configured with database cursor factories.
pub type DatabaseStorageRoot<'a, TX> =
    StorageRoot<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>;

/// Create a new [`StorageRoot`] calculator from database transaction and raw address.
pub fn storage_root_from_tx<'a, TX: DbTx>(
    tx: &'a TX,
    address: Address,
) -> DatabaseStorageRoot<'a, TX> {
    StorageRoot::new(
        DatabaseTrieCursorFactory::new(tx),
        DatabaseHashedCursorFactory::new(tx),
        address,
        Default::default(),
        #[cfg(feature = "metrics")]
        TrieRootMetrics::new(reth_trie::TrieType::Storage),
    )
}

/// Create a new [`StorageRoot`] calculator from database transaction and hashed address.
pub fn storage_root_from_tx_hashed<'a, TX: DbTx>(
    tx: &'a TX,
    hashed_address: B256,
) -> DatabaseStorageRoot<'a, TX> {
    StorageRoot::new_hashed(
        DatabaseTrieCursorFactory::new(tx),
        DatabaseHashedCursorFactory::new(tx),
        hashed_address,
        Default::default(),
        #[cfg(feature = "metrics")]
        TrieRootMetrics::new(reth_trie::TrieType::Storage),
    )
}

/// Calculates the storage root for the given [`HashedStorage`] and returns it.
pub fn storage_overlay_root<TX: DbTx>(
    tx: &TX,
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
        prefix_set,
        #[cfg(feature = "metrics")]
        TrieRootMetrics::new(reth_trie::TrieType::Storage),
    )
    .root()
}

/// Extends [`HashedStorage`] with operations specific for working with a database transaction.
pub trait DatabaseHashedStorage<TX>: Sized {
    /// Initializes [`HashedStorage`] from reverts. Iterates over storage reverts from the specified
    /// block up to the current tip and aggregates them into hashed storage in reverse.
    fn from_reverts(tx: &TX, address: Address, from: BlockNumber) -> Result<Self, DatabaseError>;
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
