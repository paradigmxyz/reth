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
use reth_trie_db::{StorageTrieEntryLike, TrieTableAdapter};

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
    let storage_cursors = tx.cursor_dup_write_shards::<tables::HashedStorages>()?;
    let mut account_trie = tx.cursor_write::<A::AccountTrieTable>()?;
    let trie_cursors = tx.cursor_dup_write_shards::<A::StorageTrieTable>()?;
    let storage_shards = storage_cursors.len();
    let trie_shards = trie_cursors.len();
    let mut results: Vec<ProviderResult<()>> =
        (0..2 + storage_shards + trie_shards).map(|_| Ok(())).collect();
    let (account_results, shard_results) = results.split_at_mut(2);
    let [accounts_result, account_trie_result] = account_results else { unreachable!() };
    let (storage_results, trie_results) = shard_results.split_at_mut(storage_shards);
    let span = tracing::Span::current();
    metrics::histogram!("storage.providers.database.persistence_worker_preparation_seconds")
        .record(preparation_started.elapsed());
    runtime.storage_pool().in_place_scope(|scope| {
        scope.spawn(|_| {
            let _guard = span.enter();
            let _timer = super::metrics::PersistenceTableTimer::new(tables::HashedAccounts::NAME, 0);
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
        for (shard, (mut storage, storage_result)) in
            storage_cursors.into_iter().zip(storage_results).enumerate()
        {
            let storages = &storages;
            let span = &span;
            scope.spawn(move |_| {
                let _guard = span.enter();
                let _timer = super::metrics::PersistenceTableTimer::new(tables::HashedStorages::NAME, shard);
                let started = std::time::Instant::now();
                *storage_result = (|| {
                    for &(address, changes) in storages {
                        for (slot, value) in changes.storage_slots_ref() {
                            if storage_shards > 1 && usize::from(slot[0] >> 6) != shard {
                                continue
                            }
                            let entry = StorageEntry { key: *slot, value: *value };
                            if let Some(existing) =
                                storage.seek_by_key_subkey(*address, entry.key)? &&
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
                tracing::debug!(target: "engine::persistence", shard,
                    table = "HashedStorages", elapsed = ?started.elapsed(), "Finished storage shard writes");
            });
        }
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
        for (shard, (mut storage_trie, storage_trie_result)) in
            trie_cursors.into_iter().zip(trie_results).enumerate()
        {
            let storage_tries = &storage_tries;
            let span = &span;
            scope.spawn(move |_| {
                let _guard = span.enter();
                let _timer = super::metrics::PersistenceTableTimer::new(A::StorageTrieTable::NAME, shard);
                let started = std::time::Instant::now();
                *storage_trie_result = (|| {
                    for &(address, updates) in storage_tries {
                        for (nibbles, node) in updates.storage_nodes_ref() {
                            if nibbles.is_empty() ||
                                (trie_shards > 1 &&
                                    usize::from(nibbles.get_unchecked(0) >> 2) != shard)
                            {
                                continue
                            }
                            let key = A::StorageSubKey::from(*nibbles);
                            if storage_trie
                                .seek_by_key_subkey(*address, key.clone())?
                                .as_ref()
                                .is_some_and(|entry| *entry.nibbles() == key)
                            {
                                storage_trie.delete_current()?;
                            }
                            if let Some(node) = node {
                                storage_trie
                                    .upsert(*address, &A::StorageValue::new(key, node.clone()))?;
                            }
                        }
                    }
                    Ok(())
                })();
                tracing::debug!(target: "engine::persistence", shard,
                    table = "StoragesTrie", elapsed = ?started.elapsed(), "Finished storage shard writes");
            });
        }
    });
    drop((accounts, account_trie));
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
