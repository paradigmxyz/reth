use std::collections::BTreeSet;

use alloy_primitives::{map::HashMap, B256};
use reth_db::DatabaseError;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, Nibbles,
};
use tracing::debug;

#[derive(Debug)]
struct EntryDiff<T> {
    task: T,
    regular: T,
    database: T,
}

#[derive(Debug)]
struct BranchNodeCompactDiff {
    task: Option<Option<BranchNodeCompact>>,
    regular: Option<Option<BranchNodeCompact>>,
    database: Option<BranchNodeCompact>,
}

#[derive(Debug, Default)]
struct TrieUpdatesDiff {
    changed_nodes: HashMap<Nibbles, BranchNodeCompactDiff>,
    storage_tries: HashMap<B256, StorageTrieDiffEntry>,
}

impl TrieUpdatesDiff {
    fn is_empty(&self) -> bool {
        self.changed_nodes.is_empty() && self.storage_tries.is_empty()
    }

    pub(super) fn log_differences(mut self) {
        if !self.is_empty() {
            for (path, BranchNodeCompactDiff { task, regular, database }) in &mut self.changed_nodes
            {
                debug!(target: "engine::tree", ?path, ?task, ?regular, ?database, "Difference in account trie updates");
            }

            for (address, storage_diff) in self.storage_tries {
                storage_diff.log_differences(address);
            }
        }
    }
}

#[derive(Debug)]
enum StorageTrieDiffEntry {
    /// Storage Trie entry exists for one of the task or regular trie updates, but not the other.
    Existence(bool, bool),
    /// Storage Trie entries exists for both task and regular trie updates, but their values
    /// differ.
    Value(StorageTrieUpdatesDiff),
}

impl StorageTrieDiffEntry {
    fn log_differences(self, address: B256) {
        match self {
            Self::Existence(task, regular) => {
                debug!(target: "engine::tree", ?address, ?task, ?regular, "Difference in storage trie existence");
            }
            Self::Value(mut storage_diff) => {
                if let Some(EntryDiff { task, regular, database }) = storage_diff.is_deleted {
                    debug!(target: "engine::tree", ?address, ?task, ?regular, ?database, "Difference in storage trie deletion");
                }

                for (path, BranchNodeCompactDiff { task, regular, database }) in
                    &mut storage_diff.changed_nodes
                {
                    debug!(target: "engine::tree", ?address, ?path, ?task, ?regular, ?database, "Difference in storage trie updates");
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct StorageTrieUpdatesDiff {
    is_deleted: Option<EntryDiff<bool>>,
    changed_nodes: HashMap<Nibbles, BranchNodeCompactDiff>,
}

impl StorageTrieUpdatesDiff {
    fn is_empty(&self) -> bool {
        self.is_deleted.is_none() && self.changed_nodes.is_empty()
    }
}

/// Compares the trie updates from state root task, regular state root calculation and database,
/// and logs the differences if there's any.
pub(super) fn compare_trie_updates(
    trie_cursor_factory: impl TrieCursorFactory,
    mut task: TrieUpdates,
    mut regular: TrieUpdates,
) -> Result<(), DatabaseError> {
    let mut diff = TrieUpdatesDiff::default();

    // compare account nodes
    let mut account_trie_cursor = trie_cursor_factory.account_trie_cursor()?;
    for key in Iterator::chain(task.changed_nodes.keys(), regular.changed_nodes.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let task = task.changed_nodes.remove(&key);
        let regular = regular.changed_nodes.remove(&key);
        let database = account_trie_cursor.seek_exact(key.clone())?.map(|x| x.1);
        if !branch_nodes_equal(&task, &regular, &database) {
            diff.changed_nodes.insert(key, BranchNodeCompactDiff { task, regular, database });
        }
    }

    for key in Iterator::chain(task.storage_tries.keys(), regular.storage_tries.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let mut task = task.storage_tries.remove(&key);
        let mut regular = regular.storage_tries.remove(&key);
        if task != regular {
            if let Some((task, regular)) = task.as_mut().zip(regular.as_mut()) {
                let storage_diff = compare_storage_trie_updates(
                    || trie_cursor_factory.storage_trie_cursor(key),
                    task,
                    regular,
                )?;
                if !storage_diff.is_empty() {
                    diff.storage_tries.insert(key, StorageTrieDiffEntry::Value(storage_diff));
                }
            } else {
                diff.storage_tries.insert(
                    key,
                    StorageTrieDiffEntry::Existence(task.is_some(), regular.is_some()),
                );
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
    let database_deleted = trie_cursor()?.next()?.is_none();
    let mut diff = StorageTrieUpdatesDiff {
        is_deleted: (task.is_deleted != regular.is_deleted).then_some(EntryDiff {
            task: task.is_deleted,
            regular: regular.is_deleted,
            database: database_deleted,
        }),
        ..Default::default()
    };

    // compare storage nodes
    let mut storage_trie_cursor = trie_cursor()?;
    for key in Iterator::chain(task.changed_nodes.keys(), regular.changed_nodes.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
    {
        let task = task.changed_nodes.remove(&key);
        let regular = regular.changed_nodes.remove(&key);
        let database = storage_trie_cursor.seek_exact(key.clone())?.map(|x| x.1);
        if !branch_nodes_equal(&task, &regular, &database) {
            diff.changed_nodes.insert(key, BranchNodeCompactDiff { task, regular, database });
        }
    }

    Ok(diff)
}

/// Compares the branch nodes from state root task and regular state root calculation.
///
/// If one of the branch nodes is [`None`], it means it's not updated and the other is compared to
/// the branch node from the database.
///
/// Returns `true` if they are equal.
fn branch_nodes_equal(
    task: &Option<Option<BranchNodeCompact>>,
    regular: &Option<Option<BranchNodeCompact>>,
    database: &Option<BranchNodeCompact>,
) -> bool {
    regular.as_ref().unwrap_or(database) == task.as_ref().unwrap_or(database)
}
