//! Builder for [`StorageAccountFilter`] from database.

use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_trie_common::StorageAccountFilter;
use std::{sync::Mutex, thread};
use tracing::{debug, info};

/// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table in parallel.
///
/// This spawns multiple threads, each with its own database transaction, to collect
/// unique hashed addresses that have storage entries. The address space is split
/// across threads for parallel processing.
///
/// Creates a cuckoo filter sized to hold all addresses with 20% headroom.
pub fn build_storage_filter_parallel<DB: Database>(
    db: &DB,
    num_threads: usize,
) -> Result<StorageAccountFilter, DatabaseError> {
    let start = std::time::Instant::now();
    let num_threads = num_threads.max(1);

    info!(
        target: "trie::storage_filter",
        num_threads,
        "Starting parallel storage filter build"
    );

    let all_addresses: Mutex<Vec<B256>> = Mutex::new(Vec::new());
    let errors: Mutex<Option<DatabaseError>> = Mutex::new(None);

    thread::scope(|scope| {
        for chunk_idx in 0..num_threads {
            let all_addresses = &all_addresses;
            let errors = &errors;

            scope.spawn(move || {
                let result = (|| -> Result<Vec<B256>, DatabaseError> {
                    let tx = db.tx()?;
                    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

                    let chunk_size = U256::MAX / U256::from(num_threads);
                    let start_val = U256::from(chunk_idx) * chunk_size;
                    let end_val = if chunk_idx == num_threads - 1 {
                        U256::MAX
                    } else {
                        U256::from(chunk_idx + 1) * chunk_size
                    };

                    let start_key = B256::from(start_val);
                    let mut local_addresses = Vec::new();

                    let mut entry = cursor.seek(start_key)?;
                    while let Some((address, _)) = entry {
                        let addr_val = U256::from_be_bytes(address.0);
                        if addr_val >= end_val {
                            break;
                        }
                        local_addresses.push(address);
                        entry = cursor.next_no_dup()?;
                    }

                    debug!(
                        target: "trie::storage_filter",
                        chunk_idx,
                        count = local_addresses.len(),
                        "Thread collected addresses"
                    );

                    Ok(local_addresses)
                })();

                match result {
                    Ok(local_addresses) => {
                        all_addresses.lock().unwrap().extend(local_addresses);
                    }
                    Err(e) => {
                        let mut guard = errors.lock().unwrap();
                        if guard.is_none() {
                            *guard = Some(e);
                        }
                    }
                }
            });
        }
    });

    if let Some(e) = errors.into_inner().unwrap() {
        return Err(e);
    }

    let addresses = all_addresses.into_inner().unwrap();
    let count = addresses.len();
    let capacity = (count as f64 * 1.2) as usize;

    debug!(
        target: "trie::storage_filter",
        count,
        capacity,
        elapsed_collect_ms = start.elapsed().as_millis(),
        "Collected hashed addresses with storage"
    );

    let mut filter = StorageAccountFilter::with_capacity(capacity.max(1));
    let mut insert_failures = 0usize;

    for address in addresses {
        if filter.insert(address).is_err() {
            insert_failures += 1;
        }
    }

    if insert_failures > 0 {
        debug!(
            target: "trie::storage_filter",
            insert_failures,
            "Some addresses failed to insert into filter"
        );
    }

    info!(
        target: "trie::storage_filter",
        count = filter.len(),
        memory_bytes = filter.memory_usage(),
        memory_mb = format!("{:.2}", filter.memory_usage() as f64 / 1024.0 / 1024.0),
        elapsed_ms = start.elapsed().as_millis(),
        "Built storage account filter"
    );

    Ok(filter)
}

/// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table.
///
/// This collects all unique hashed addresses that have storage entries and
/// creates a cuckoo filter sized to hold them with 20% headroom.
///
/// For parallel execution with multiple threads, use [`build_storage_filter_parallel`].
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

/// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table in parallel.
///
/// This spawns multiple threads, each creating its own database provider/transaction
/// from the factory, to collect unique hashed addresses that have storage entries.
/// The address space is split across threads for parallel processing.
///
/// Creates a cuckoo filter sized to hold all addresses with 20% headroom.
pub fn build_storage_filter_parallel_from_factory<F>(
    factory: &F,
    num_threads: usize,
) -> Result<StorageAccountFilter, ProviderError>
where
    F: DatabaseProviderFactory + Sync,
    <F as DatabaseProviderFactory>::Provider: DBProvider,
{
    let start = std::time::Instant::now();
    let num_threads = num_threads.max(1);

    info!(
        target: "trie::storage_filter",
        num_threads,
        "Starting parallel storage filter build"
    );

    let all_addresses: Mutex<Vec<B256>> = Mutex::new(Vec::new());
    let errors: Mutex<Option<ProviderError>> = Mutex::new(None);

    thread::scope(|scope| {
        for chunk_idx in 0..num_threads {
            let all_addresses = &all_addresses;
            let errors = &errors;

            scope.spawn(move || {
                let result = (|| -> Result<Vec<B256>, ProviderError> {
                    let provider = factory.database_provider_ro()?;
                    let tx = provider.tx_ref();
                    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

                    let chunk_size = U256::MAX / U256::from(num_threads);
                    let start_val = U256::from(chunk_idx) * chunk_size;
                    let end_val = if chunk_idx == num_threads - 1 {
                        U256::MAX
                    } else {
                        U256::from(chunk_idx + 1) * chunk_size
                    };

                    let start_key = B256::from(start_val);
                    let mut local_addresses = Vec::new();

                    let mut entry = cursor.seek(start_key)?;
                    while let Some((address, _)) = entry {
                        let addr_val = U256::from_be_bytes(address.0);
                        if addr_val >= end_val {
                            break;
                        }
                        local_addresses.push(address);
                        entry = cursor.next_no_dup()?;
                    }

                    debug!(
                        target: "trie::storage_filter",
                        chunk_idx,
                        count = local_addresses.len(),
                        "Thread collected addresses"
                    );

                    Ok(local_addresses)
                })();

                match result {
                    Ok(local_addresses) => {
                        all_addresses.lock().unwrap().extend(local_addresses);
                    }
                    Err(e) => {
                        let mut guard = errors.lock().unwrap();
                        if guard.is_none() {
                            *guard = Some(e);
                        }
                    }
                }
            });
        }
    });

    if let Some(e) = errors.into_inner().unwrap() {
        return Err(e);
    }

    let addresses = all_addresses.into_inner().unwrap();
    let count = addresses.len();
    let capacity = (count as f64 * 1.2) as usize;

    debug!(
        target: "trie::storage_filter",
        count,
        capacity,
        elapsed_collect_ms = start.elapsed().as_millis(),
        "Collected hashed addresses with storage"
    );

    let mut filter = StorageAccountFilter::with_capacity(capacity.max(1));
    let mut insert_failures = 0usize;

    for address in addresses {
        if filter.insert(address).is_err() {
            insert_failures += 1;
        }
    }

    if insert_failures > 0 {
        debug!(
            target: "trie::storage_filter",
            insert_failures,
            "Some addresses failed to insert into filter"
        );
    }

    info!(
        target: "trie::storage_filter",
        count = filter.len(),
        memory_bytes = filter.memory_usage(),
        memory_mb = format!("{:.2}", filter.memory_usage() as f64 / 1024.0 / 1024.0),
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

/// Extension trait for building [`StorageAccountFilter`] from a database provider factory in
/// parallel.
pub trait StorageFilterFactoryBuilder {
    /// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table in parallel.
    ///
    /// Uses multiple threads with dedicated database providers to speed up collection.
    fn build_storage_filter_parallel(
        &self,
        num_threads: usize,
    ) -> Result<StorageAccountFilter, ProviderError>;
}

impl<F> StorageFilterFactoryBuilder for F
where
    F: DatabaseProviderFactory + Sync,
    <F as DatabaseProviderFactory>::Provider: DBProvider,
{
    fn build_storage_filter_parallel(
        &self,
        num_threads: usize,
    ) -> Result<StorageAccountFilter, ProviderError> {
        build_storage_filter_parallel_from_factory(self, num_threads)
    }
}

/// Extension trait for building [`StorageAccountFilter`] from a database in parallel.
pub trait StorageFilterParallelBuilder {
    /// Builds a [`StorageAccountFilter`] by walking the `HashedStorages` table in parallel.
    ///
    /// Uses multiple threads with dedicated database transactions to speed up collection.
    fn build_storage_filter_parallel(
        &self,
        num_threads: usize,
    ) -> Result<StorageAccountFilter, DatabaseError>;

    /// Returns the default number of threads for parallel filter building.
    ///
    /// Defaults to the number of available CPU cores, or 1 if unavailable.
    fn default_parallel_threads() -> usize {
        thread::available_parallelism().map(|p| p.get()).unwrap_or(1)
    }
}

impl<DB: Database> StorageFilterParallelBuilder for DB {
    fn build_storage_filter_parallel(
        &self,
        num_threads: usize,
    ) -> Result<StorageAccountFilter, DatabaseError> {
        build_storage_filter_parallel(self, num_threads)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_build_empty_filter() {
        // This test requires a mock database, which we skip for now
        // The actual integration test would go in reth-trie-db tests
    }
}
