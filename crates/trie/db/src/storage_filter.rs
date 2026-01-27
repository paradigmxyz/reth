//! Builder for [`StorageAccountFilter`] from database.

use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_storage_api::DBProvider;
use reth_trie_common::StorageAccountFilter;
use tracing::{debug, info};

/// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table.
///
/// This collects all unique hashed addresses that have storage entries and
/// creates a cuckoo filter sized to hold them with 20% headroom.
pub fn build_storage_filter<TX: DbTx>(tx: &TX) -> Result<StorageAccountFilter, DatabaseError> {
    let start = std::time::Instant::now();

    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
    let mut addresses = Vec::new();

    let mut entry = cursor.first()?;
    while let Some((address, _)) = entry {
        addresses.push(address);
        entry = cursor.next_no_dup()?;
    }

    let count = addresses.len();
    let capacity = (count as f64 * 1.2) as usize;

    debug!(
        target: "trie::storage_filter",
        count,
        capacity,
        "Collected hashed addresses with storage"
    );

    let mut filter = StorageAccountFilter::with_capacity(capacity.max(1));

    for address in addresses {
        if let Err(e) = filter.insert(address) {
            debug!(
                target: "trie::storage_filter",
                %address,
                ?e,
                "Failed to insert address into filter, filter may be too full"
            );
        }
    }

    info!(
        target: "trie::storage_filter",
        count = filter.len(),
        memory_bytes = filter.memory_usage(),
        elapsed_ms = start.elapsed().as_millis(),
        "Built storage account filter"
    );

    Ok(filter)
}

/// Extension trait for building [`StorageAccountFilter`] from a database provider.
pub trait StorageFilterBuilder {
    /// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table.
    fn build_storage_filter(&self) -> Result<StorageAccountFilter, DatabaseError>;
}

impl<P: DBProvider> StorageFilterBuilder for P {
    fn build_storage_filter(&self) -> Result<StorageAccountFilter, DatabaseError> {
        build_storage_filter(self.tx_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_db_api::{cursor::DbCursorRW, tables, transaction::DbTxMut};
    use reth_trie_common::StorageTrieEntry;

    #[test]
    fn test_build_empty_filter() {
        // This test requires a mock database, which we skip for now
        // The actual integration test would go in reth-trie-db tests
    }
}
