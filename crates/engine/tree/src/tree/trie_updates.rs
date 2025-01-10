use alloy_primitives::{
    map::{HashMap, HashSet},
    B256,
};
use reth_db::{transaction::DbTx, DatabaseError};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, Nibbles,
};
use reth_trie_db::DatabaseTrieCursorFactory;
use tracing::debug;

#[derive(Debug, Default)]
struct TrieUpdatesDiff {
    account_nodes: HashMap<Nibbles, (Option<BranchNodeCompact>, Option<BranchNodeCompact>)>,
    removed_nodes: HashMap<Nibbles, (bool, bool)>,
    storage_tries: HashMap<B256, StorageTrieDiffEntry>,
}

impl TrieUpdatesDiff {
    fn has_differences(&self) -> bool {
        !self.account_nodes.is_empty() ||
            !self.removed_nodes.is_empty() ||
            !self.storage_tries.is_empty()
    }

    pub(super) fn log_differences(mut self) {
        if self.has_differences() {
            for (path, (task, regular)) in &mut self.account_nodes {
                debug!(target: "engine::tree", ?path, ?task, ?regular, "Difference in account trie updates");
            }

            for (path, (task, regular)) in &self.removed_nodes {
                debug!(target: "engine::tree", ?path, ?task, ?regular, "Difference in removed account trie nodes");
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
                if let Some((task, regular)) = storage_diff.is_deleted {
                    debug!(target: "engine::tree", ?address, ?task, ?regular, "Difference in storage trie deletion");
                }

                for (path, (task, regular)) in &mut storage_diff.storage_nodes {
                    debug!(target: "engine::tree", ?address, ?path, ?task, ?regular, "Difference in storage trie updates");
                }

                for (path, (task, regular)) in &storage_diff.removed_nodes {
                    debug!(target: "engine::tree", ?address, ?path, ?task, ?regular, "Difference in removed account trie nodes");
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct StorageTrieUpdatesDiff {
    is_deleted: Option<(bool, bool)>,
    storage_nodes: HashMap<Nibbles, (Option<BranchNodeCompact>, Option<BranchNodeCompact>)>,
    removed_nodes: HashMap<Nibbles, (bool, bool)>,
}

impl StorageTrieUpdatesDiff {
    fn has_differences(&self) -> bool {
        self.is_deleted.is_some() ||
            !self.storage_nodes.is_empty() ||
            !self.removed_nodes.is_empty()
    }
}

/// Compares the trie updates from state root task and regular state root calculation, and logs
/// the differences if there's any.
pub(super) fn compare_trie_updates(
    tx: &impl DbTx,
    task: TrieUpdates,
    regular: TrieUpdates,
) -> Result<(), DatabaseError> {
    let trie_cursor_factory = DatabaseTrieCursorFactory::new(tx);

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
        .collect::<HashSet<_>>()
    {
        let (left, right) = (task.account_nodes.remove(&key), regular.account_nodes.remove(&key));

        if !branch_nodes_equal(&mut account_trie_cursor, &key, left.as_ref(), right.as_ref())? {
            diff.account_nodes.insert(key, (left, right));
        }
    }

    // compare removed nodes
    for key in task
        .removed_nodes
        .iter()
        .chain(regular.removed_nodes.iter())
        .cloned()
        .collect::<HashSet<_>>()
    {
        let (left, right) =
            (task.removed_nodes.contains(&key), regular.removed_nodes.contains(&key));
        if left != right && account_trie_cursor.seek_exact(key.clone())?.is_some() {
            diff.removed_nodes.insert(key, (left, right));
        }
    }

    // compare storage tries
    for key in task
        .storage_tries
        .keys()
        .chain(regular.storage_tries.keys())
        .copied()
        .collect::<HashSet<_>>()
    {
        let (task, regular) = (task.storage_tries.remove(&key), regular.storage_tries.remove(&key));
        if task != regular {
            let mut storage_trie_cursor = trie_cursor_factory.storage_trie_cursor(key)?;
            if task.is_some() != regular.is_some() {
                diff.storage_tries.insert(
                    key,
                    StorageTrieDiffEntry::Existence(task.is_some(), regular.is_some()),
                );
            }

            let storage_diff = compare_storage_trie_updates(
                &mut storage_trie_cursor,
                &task.unwrap_or_default(),
                &regular.unwrap_or_default(),
            )?;
            if storage_diff.has_differences() {
                diff.storage_tries.insert(key, StorageTrieDiffEntry::Value(storage_diff));
            }
        }
    }

    // log differences
    diff.log_differences();

    Ok(())
}

fn compare_storage_trie_updates(
    trie_cursor: &mut impl TrieCursor,
    task: &StorageTrieUpdates,
    regular: &StorageTrieUpdates,
) -> Result<StorageTrieUpdatesDiff, DatabaseError> {
    let mut diff = StorageTrieUpdatesDiff {
        is_deleted: (task.is_deleted != regular.is_deleted)
            .then_some((task.is_deleted, regular.is_deleted)),
        ..Default::default()
    };

    // compare storage nodes
    for key in task
        .storage_nodes
        .keys()
        .chain(regular.storage_nodes.keys())
        .cloned()
        .collect::<HashSet<_>>()
    {
        let (left, right) = (task.storage_nodes.get(&key), regular.storage_nodes.get(&key));
        if !branch_nodes_equal(trie_cursor, &key, left, right)? {
            diff.storage_nodes.insert(key, (left.cloned(), right.cloned()));
        }
    }

    // compare removed nodes
    for key in task
        .removed_nodes
        .iter()
        .chain(regular.removed_nodes.iter())
        .cloned()
        .collect::<HashSet<_>>()
    {
        let (left, right) =
            (task.removed_nodes.contains(&key), regular.removed_nodes.contains(&key));
        if left != right && trie_cursor.seek_exact(key.clone())?.is_some() {
            diff.removed_nodes.insert(key, (left, right));
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
    trie_cursor: &mut impl TrieCursor,
    key: &Nibbles,
    task: Option<&BranchNodeCompact>,
    regular: Option<&BranchNodeCompact>,
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
            let db_branch_node = trie_cursor.seek_exact(key.clone())?.map(|x| x.1);
            if task.is_some() {
                task == db_branch_node.as_ref()
            } else {
                regular == db_branch_node.as_ref()
            }
        }
    })
}
