use std::collections::BTreeSet;

use alloy_primitives::{map::HashMap, B256};
use reth_db::DatabaseError;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, Nibbles,
};
use tracing::warn;

#[derive(Debug)]
struct EntryDiff<T> {
    task: T,
    regular: T,
    database: T,
}

#[derive(Debug, Default)]
struct TrieUpdatesDiff {
    account_nodes: HashMap<Nibbles, EntryDiff<Option<BranchNodeCompact>>>,
    removed_nodes: HashMap<Nibbles, EntryDiff<bool>>,
    storage_tries: HashMap<B256, StorageTrieUpdatesDiff>,
}

impl TrieUpdatesDiff {
    fn has_differences(&self) -> bool {
        !self.account_nodes.is_empty() ||
            !self.removed_nodes.is_empty() ||
            !self.storage_tries.is_empty()
    }

    pub(super) fn log_differences(mut self) {
        if self.has_differences() {
            for (path, EntryDiff { task, regular, database }) in &mut self.account_nodes {
                warn!(target: "engine::tree", ?path, ?task, ?regular, ?database, "Difference in account trie updates");
            }

            for (
                path,
                EntryDiff {
                    task: task_removed,
                    regular: regular_removed,
                    database: database_not_exists,
                },
            ) in &self.removed_nodes
            {
                warn!(target: "engine::tree", ?path, ?task_removed, ?regular_removed, ?database_not_exists, "Difference in removed account trie nodes");
            }

            for (address, storage_diff) in self.storage_tries {
                storage_diff.log_differences(address);
            }
        }
    }
}

#[derive(Debug, Default)]
struct StorageTrieUpdatesDiff {
    is_deleted: Option<EntryDiff<bool>>,
    storage_nodes: HashMap<Nibbles, EntryDiff<Option<BranchNodeCompact>>>,
    removed_nodes: HashMap<Nibbles, EntryDiff<bool>>,
}

impl StorageTrieUpdatesDiff {
    fn has_differences(&self) -> bool {
        self.is_deleted.is_some() ||
            !self.storage_nodes.is_empty() ||
            !self.removed_nodes.is_empty()
    }

    fn log_differences(&self, address: B256) {
        if let Some(EntryDiff {
            task: task_deleted,
            regular: regular_deleted,
            database: database_not_exists,
        }) = self.is_deleted
        {
            warn!(target: "engine::tree", ?address, ?task_deleted, ?regular_deleted, ?database_not_exists, "Difference in storage trie deletion");
        }

        for (path, EntryDiff { task, regular, database }) in &self.storage_nodes {
            warn!(target: "engine::tree", ?address, ?path, ?task, ?regular, ?database, "Difference in storage trie updates");
        }

        for (
            path,
            EntryDiff {
                task: task_removed,
                regular: regular_removed,
                database: database_not_exists,
            },
        ) in &self.removed_nodes
        {
            warn!(target: "engine::tree", ?address, ?path, ?task_removed, ?regular_removed, ?database_not_exists, "Difference in removed storage trie nodes");
        }
    }
}

/// Compares the trie updates from state root task, regular state root calculation and database,
/// and logs the differences if there's any.
pub(super) fn compare_trie_updates(
    trie_cursor_factory: impl TrieCursorFactory,
    task: TrieUpdates,
    regular: TrieUpdates,
) -> Result<(), DatabaseError> {
    let mut task = adjust_trie_updates(task);
    let mut regular = adjust_trie_updates(regular);

    let mut diff = TrieUpdatesDiff::default();

    // compare account nodes
    let mut account_trie_cursor = trie_cursor_factory.account_trie_cursor()?;
    for key in task
        .account_nodes
        .keys()
        .chain(regular.account_nodes.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let (task, regular) = (task.account_nodes.remove(&key), regular.account_nodes.remove(&key));
        let database = account_trie_cursor.seek_exact(key.clone())?.map(|x| x.1);

        if !branch_nodes_equal(task.as_ref(), regular.as_ref(), database.as_ref())? {
            diff.account_nodes.insert(key, EntryDiff { task, regular, database });
        }
    }

    // compare removed nodes
    let mut account_trie_cursor = trie_cursor_factory.account_trie_cursor()?;
    for key in task
        .removed_nodes
        .iter()
        .chain(regular.removed_nodes.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let (task_removed, regular_removed) =
            (task.removed_nodes.contains(&key), regular.removed_nodes.contains(&key));
        let database_not_exists = account_trie_cursor.seek_exact(key.clone())?.is_none();
        // If the deletion is a no-op, meaning that the entry is not in the
        // database, do not add it to the diff.
        if task_removed != regular_removed && !database_not_exists {
            diff.removed_nodes.insert(
                key,
                EntryDiff {
                    task: task_removed,
                    regular: regular_removed,
                    database: database_not_exists,
                },
            );
        }
    }

    // compare storage tries
    for key in task
        .storage_tries
        .keys()
        .chain(regular.storage_tries.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let (mut task, mut regular) =
            (task.storage_tries.remove(&key), regular.storage_tries.remove(&key));
        if task != regular {
            #[allow(clippy::or_fun_call)]
            let storage_diff = compare_storage_trie_updates(
                || trie_cursor_factory.storage_trie_cursor(key),
                // Compare non-existent storage tries as empty.
                task.as_mut().unwrap_or(&mut Default::default()),
                regular.as_mut().unwrap_or(&mut Default::default()),
            )?;
            if storage_diff.has_differences() {
                diff.storage_tries.insert(key, storage_diff);
            }
        }
    }

    // log differences
    diff.log_differences();

    Ok(())
}

fn compare_storage_trie_updates<C: TrieCursor>(
    trie_cursor: impl Fn() -> Result<C, DatabaseError>,
    task: &mut StorageTrieUpdates,
    regular: &mut StorageTrieUpdates,
) -> Result<StorageTrieUpdatesDiff, DatabaseError> {
    let database_not_exists = trie_cursor()?.next()?.is_none();
    let mut diff = StorageTrieUpdatesDiff {
        // If the deletion is a no-op, meaning that the entry is not in the
        // database, do not add it to the diff.
        is_deleted: (task.is_deleted != regular.is_deleted && !database_not_exists).then_some(
            EntryDiff {
                task: task.is_deleted,
                regular: regular.is_deleted,
                database: database_not_exists,
            },
        ),
        ..Default::default()
    };

    // compare storage nodes
    let mut storage_trie_cursor = trie_cursor()?;
    for key in task
        .storage_nodes
        .keys()
        .chain(regular.storage_nodes.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let (task, regular) = (task.storage_nodes.remove(&key), regular.storage_nodes.remove(&key));
        let database = storage_trie_cursor.seek_exact(key.clone())?.map(|x| x.1);
        if !branch_nodes_equal(task.as_ref(), regular.as_ref(), database.as_ref())? {
            diff.storage_nodes.insert(key, EntryDiff { task, regular, database });
        }
    }

    // compare removed nodes
    let mut storage_trie_cursor = trie_cursor()?;
    for key in task
        .removed_nodes
        .iter()
        .chain(regular.removed_nodes.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let (task_removed, regular_removed) =
            (task.removed_nodes.contains(&key), regular.removed_nodes.contains(&key));
        let database_not_exists =
            storage_trie_cursor.seek_exact(key.clone())?.map(|x| x.1).is_none();
        // If the deletion is a no-op, meaning that the entry is not in the
        // database, do not add it to the diff.
        if task_removed != regular_removed && !database_not_exists {
            diff.removed_nodes.insert(
                key,
                EntryDiff {
                    task: task_removed,
                    regular: regular_removed,
                    database: database_not_exists,
                },
            );
        }
    }

    Ok(diff)
}

/// Filters the removed nodes of both account trie updates and storage trie updates, so that they
/// don't include those nodes that were also updated.
fn adjust_trie_updates(trie_updates: TrieUpdates) -> TrieUpdates {
    TrieUpdates {
        removed_nodes: trie_updates
            .removed_nodes
            .into_iter()
            .filter(|key| !trie_updates.account_nodes.contains_key(key))
            .collect(),
        storage_tries: trie_updates
            .storage_tries
            .into_iter()
            .map(|(address, updates)| {
                (
                    address,
                    StorageTrieUpdates {
                        removed_nodes: updates
                            .removed_nodes
                            .into_iter()
                            .filter(|key| !updates.storage_nodes.contains_key(key))
                            .collect(),
                        ..updates
                    },
                )
            })
            .collect(),
        ..trie_updates
    }
}

/// Compares the branch nodes from state root task and regular state root calculation.
///
/// If one of the branch nodes is [`None`], it means it's not updated and the other is compared to
/// the branch node from the database.
///
/// Returns `true` if they are equal.
fn branch_nodes_equal(
    task: Option<&BranchNodeCompact>,
    regular: Option<&BranchNodeCompact>,
    database: Option<&BranchNodeCompact>,
) -> Result<bool, DatabaseError> {
    Ok(match (task, regular) {
        (Some(task), Some(regular)) => {
            task.state_mask == regular.state_mask &&
                task.tree_mask == regular.tree_mask &&
                task.hash_mask == regular.hash_mask &&
                task.hashes == regular.hashes &&
                task.root_hash == regular.root_hash
        }
        (None, None) => true,
        _ => {
            if task.is_some() {
                task == database
            } else {
                regular == database
            }
        }
    })
}
