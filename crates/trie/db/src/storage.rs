use reth_db_api::transaction::DbTx;
use reth_primitives::{Address, B256};
use reth_trie::{hashed_cursor::DatabaseHashedCursorFactory, StorageRoot};

#[cfg(feature = "metrics")]
use reth_trie::metrics::{TrieRootMetrics, TrieType};

/// Extends [`StorageRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStorageRoot<'a, TX> {
    /// Create a new storage root calculator from database transaction and raw address.
    fn from_tx(tx: &'a TX, address: Address) -> Self;

    /// Create a new storage root calculator from database transaction and hashed address.
    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self;
}

impl<'a, TX: DbTx> DatabaseStorageRoot<'a, TX>
    for StorageRoot<&'a TX, DatabaseHashedCursorFactory<'a, TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            tx,
            DatabaseHashedCursorFactory::new(tx),
            address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }

    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self {
        Self::new_hashed(
            tx,
            DatabaseHashedCursorFactory::new(tx),
            hashed_address,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(TrieType::Storage),
        )
    }
}
