use alloy_primitives::{
    map::{HashMap, HashSet},
    B256,
};
use reth_trie::{
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, Nibbles,
};
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
pub(super) fn compare_trie_updates(task: &TrieUpdates, regular: &TrieUpdates) {
    let mut diff = TrieUpdatesDiff::default();

    // compare account nodes
    for key in task
        .account_nodes
        .keys()
        .chain(regular.account_nodes.keys())
        .cloned()
        .collect::<HashSet<_>>()
    {
        let (left, right) = (task.account_nodes.get(&key), regular.account_nodes.get(&key));

        if !branch_nodes_equal(left, right) {
            diff.account_nodes.insert(key, (left.cloned(), right.cloned()));
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
        if left != right {
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
        let (left, right) = (task.storage_tries.get(&key), regular.storage_tries.get(&key));
        if left != right {
            if let Some((left, right)) = left.zip(right) {
                let storage_diff = compare_storage_trie_updates(left, right);
                if storage_diff.has_differences() {
                    diff.storage_tries.insert(key, StorageTrieDiffEntry::Value(storage_diff));
                }
            } else {
                diff.storage_tries
                    .insert(key, StorageTrieDiffEntry::Existence(left.is_some(), right.is_some()));
            }
        }
    }

    // log differences
    diff.log_differences();
}

fn compare_storage_trie_updates(
    task: &StorageTrieUpdates,
    regular: &StorageTrieUpdates,
) -> StorageTrieUpdatesDiff {
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
        if !branch_nodes_equal(left, right) {
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
        if left != right {
            diff.removed_nodes.insert(key, (left, right));
        }
    }

    diff
}

/// Compares the branch nodes from state root task and regular state root calculation.
///
/// Returns `true` if they are equal.
fn branch_nodes_equal(
    task: Option<&BranchNodeCompact>,
    regular: Option<&BranchNodeCompact>,
) -> bool {
    if let (Some(task), Some(regular)) = (task.as_ref(), regular.as_ref()) {
        task.state_mask == regular.state_mask &&
        // We do not compare the tree mask because it is known to be mismatching
            task.hash_mask == regular.hash_mask &&
            task.hashes == regular.hashes &&
            task.root_hash == regular.root_hash
    } else {
        task == regular
    }
}
