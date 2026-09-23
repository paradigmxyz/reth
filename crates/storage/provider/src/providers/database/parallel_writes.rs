//! Experimental per-table MDBX persistence, with one cursor per worker.

use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    table::Table,
    tables,
    transaction::DbTxMut,
};
use reth_primitives_traits::StorageEntry;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted};
use reth_trie_db::{DatabaseStorageTrieCursor, TrieTableAdapter};

/// Joins every worker before merging the children into their single parent transaction.
/// Block metadata, bytecode and history writes must occur outside this scope.
pub(super) fn write_parallel<TX: DbTxMut, A: TrieTableAdapter>(
    tx: &TX,
    runtime: &reth_tasks::Runtime,
    state: &HashedPostStateSorted,
    trie: &TrieUpdatesSorted,
) -> ProviderResult<()> {
    let mut storages: Vec<_> = state.account_storages().iter().collect();
    let preparation_started = std::time::Instant::now();
    storages.sort_unstable_by_key(|(address, _)| **address);
    let mut storage_tries: Vec<_> = trie.storage_tries_ref().iter().collect();
    storage_tries.sort_unstable_by_key(|(address, _)| **address);

    // Keep the prototype's arena estimates, adjusted to the tables actually written by v2.
    let storage_count: usize = storages.iter().map(|(_, s)| s.storage_slots_ref().len()).sum();
    let trie_count: usize = storage_tries.iter().map(|(_, t)| t.storage_nodes_ref().len()).sum();
    tx.enable_parallel_writes_for_tables_with_hints(&[
        (tables::HashedAccounts::NAME, (state.accounts().len() * 2 + 8).max(3400)),
        (tables::HashedStorages::NAME, (storage_count * 3 + 8).max(4000)),
        (A::AccountTrieTable::NAME, (trie.account_nodes_ref().len() * 2 + 8).max(7300)),
        (A::StorageTrieTable::NAME, (trie_count * 3 + 8).max(8800)),
    ])?;

    // Cursors are created before workers start. No worker accesses the parent transaction.
    let mut accounts = tx.cursor_write::<tables::HashedAccounts>()?;
    let mut storage = tx.cursor_dup_write::<tables::HashedStorages>()?;
    let mut account_trie = tx.cursor_write::<A::AccountTrieTable>()?;
    let mut storage_trie = tx.cursor_dup_write::<A::StorageTrieTable>()?;
    let mut results: [ProviderResult<()>; 4] = [Ok(()), Ok(()), Ok(()), Ok(())];
    let [accounts_result, storage_result, account_trie_result, storage_trie_result] = &mut results;
    let span = tracing::Span::current();
    metrics::histogram!("storage.providers.database.persistence_worker_preparation_seconds")
        .record(preparation_started.elapsed());
    runtime.storage_pool().in_place_scope(|scope| {
        scope.spawn(|_| {
            let _guard = span.enter();
            let _timer =
                super::metrics::PersistenceTableTimer::new(tables::HashedAccounts::NAME, 0);
            *accounts_result = (|| {
                for (address, account) in state.accounts() {
                    if let Some(account) = account {
                        accounts.upsert(*address, account)?;
                    } else if accounts.seek_exact(*address)?.is_some() {
                        accounts.delete_current()?;
                    }
                }
                Ok(())
            })();
        });
        scope.spawn(|_| {
            let _guard = span.enter();
            let _timer =
                super::metrics::PersistenceTableTimer::new(tables::HashedStorages::NAME, 0);
            *storage_result = (|| {
                for (address, changes) in storages {
                    for (slot, value) in changes.storage_slots_ref() {
                        let entry = StorageEntry { key: *slot, value: *value };
                        if let Some(existing) = storage.seek_by_key_subkey(*address, entry.key)? &&
                            existing.key == entry.key
                        {
                            storage.delete_current()?;
                        }
                        if !value.is_zero() {
                            storage.upsert(*address, &entry)?;
                        }
                    }
                }
                Ok(())
            })();
        });
        scope.spawn(|_| {
            let _guard = span.enter();
            let _timer = super::metrics::PersistenceTableTimer::new(A::AccountTrieTable::NAME, 0);
            *account_trie_result = (|| {
                for (key, node) in trie.account_nodes_ref() {
                    let encoded = A::AccountKey::from(*key);
                    if let Some(node) = node {
                        if !key.is_empty() {
                            account_trie.upsert(encoded, node)?;
                        }
                    } else if account_trie.seek_exact(encoded)?.is_some() {
                        account_trie.delete_current()?;
                    }
                }
                Ok(())
            })();
        });
        scope.spawn(|_| {
            let _guard = span.enter();
            let _timer = super::metrics::PersistenceTableTimer::new(A::StorageTrieTable::NAME, 0);
            *storage_trie_result = (|| {
                for (address, updates) in storage_tries {
                    let mut cursor: DatabaseStorageTrieCursor<_, A> =
                        DatabaseStorageTrieCursor::new(storage_trie, *address);
                    cursor.write_storage_trie_updates_sorted(updates)?;
                    storage_trie = cursor.cursor;
                }
                Ok(())
            })();
        });
    });
    drop((accounts, storage, account_trie));
    // The storage-trie worker owns and drops its cursor, including on an error.
    for result in results {
        result?;
    }
    let child_commit_started = std::time::Instant::now();
    tx.commit_subtxns_with_metrics()?;
    metrics::histogram!("storage.providers.database.persistence_child_commit_seconds")
        .record(child_commit_started.elapsed());
    Ok(())
}
