use alloy_primitives::{keccak256, map::hash_map, Address, BlockNumber, B256};
use reth_db_api::{
    cursor::DbCursorRO, models::BlockNumberAddress, tables, transaction::DbTx, DatabaseError,
};
use reth_execution_errors::StorageRootError;
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::TrieCursorFactory,
    HashedPostState, HashedStorage, StorageRoot,
};

#[cfg(feature = "metrics")]
use reth_trie::metrics::TrieRootMetrics;

/// Extends [`StorageRoot`] with operations specific for working with a database provider.
pub trait DatabaseStorageRoot<'a, P> {
    /// The provider type.
    type Provider;

    /// Create a new storage root calculator from database provider and raw address.
    fn from_provider(provider: &'a Self::Provider, address: Address) -> Self;

    /// Create a new storage root calculator from database provider and hashed address.
    fn from_provider_hashed(provider: &'a Self::Provider, hashed_address: B256) -> Self;

    /// Calculates the storage root for this [`HashedStorage`] and returns it.
    fn overlay_root(
        provider: &'a P,
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

impl<'a, P> DatabaseStorageRoot<'a, P> for StorageRoot<&'a P, &'a P>
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    type Provider = P;

    fn from_provider(provider: &'a Self::Provider, address: Address) -> Self {
        Self::new(
            provider,
            provider,
            address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn from_provider_hashed(provider: &'a Self::Provider, hashed_address: B256) -> Self {
        Self::new_hashed(
            provider,
            provider,
            hashed_address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn overlay_root(
        provider: &'a P,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            provider,
            HashedPostStateCursorFactory::new(provider, &state_sorted),
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
