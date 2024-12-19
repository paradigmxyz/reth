use alloy_primitives::{
    map::{HashMap, HashSet},
    B256,
};
use reth_trie::{
    updates::{StorageTrieUpdates, TrieUpdates},
    Nibbles,
};

#[derive(Debug, Default)]
pub struct TrieUpdatesDiff {
    pub account_nodes_only_in_first: HashSet<Nibbles>,
    pub account_nodes_only_in_second: HashSet<Nibbles>,
    pub account_nodes_with_different_values: HashSet<Nibbles>,
    pub removed_nodes_only_in_first: HashSet<Nibbles>,
    pub removed_nodes_only_in_second: HashSet<Nibbles>,
    pub storage_tries_only_in_first: HashSet<B256>,
    pub storage_tries_only_in_second: HashSet<B256>,
    pub storage_tries_with_differences: HashMap<B256, StorageTrieUpdatesDiff>,
}

impl TrieUpdatesDiff {
    pub fn has_differences(&self) -> bool {
        !self.account_nodes_only_in_first.is_empty() ||
            !self.account_nodes_only_in_second.is_empty() ||
            !self.account_nodes_with_different_values.is_empty() ||
            !self.removed_nodes_only_in_first.is_empty() ||
            !self.removed_nodes_only_in_second.is_empty() ||
            !self.storage_tries_only_in_first.is_empty() ||
            !self.storage_tries_only_in_second.is_empty() ||
            !self.storage_tries_with_differences.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct StorageTrieUpdatesDiff {
    pub is_deleted_differs: bool,
    pub storage_nodes_only_in_first: HashSet<Nibbles>,
    pub storage_nodes_only_in_second: HashSet<Nibbles>,
    pub storage_nodes_with_different_values: HashSet<Nibbles>,
    pub removed_nodes_only_in_first: HashSet<Nibbles>,
    pub removed_nodes_only_in_second: HashSet<Nibbles>,
}

impl StorageTrieUpdatesDiff {
    fn has_differences(&self) -> bool {
        self.is_deleted_differs ||
            !self.storage_nodes_only_in_first.is_empty() ||
            !self.storage_nodes_only_in_second.is_empty() ||
            !self.storage_nodes_with_different_values.is_empty() ||
            !self.removed_nodes_only_in_first.is_empty() ||
            !self.removed_nodes_only_in_second.is_empty()
    }
}

pub fn compare_trie_updates(first: &TrieUpdates, second: &TrieUpdates) -> TrieUpdatesDiff {
    let mut diff = TrieUpdatesDiff::default();

    // compare account nodes
    for key in first.account_nodes.keys() {
        if !second.account_nodes.contains_key(key) {
            diff.account_nodes_only_in_first.insert(key.clone());
        } else if first.account_nodes.get(key) != second.account_nodes.get(key) {
            diff.account_nodes_with_different_values.insert(key.clone());
        }
    }
    for key in second.account_nodes.keys() {
        if !first.account_nodes.contains_key(key) {
            diff.account_nodes_only_in_second.insert(key.clone());
        }
    }

    // compare removed nodes
    for node in &first.removed_nodes {
        if !second.removed_nodes.contains(node) {
            diff.removed_nodes_only_in_first.insert(node.clone());
        }
    }
    for node in &second.removed_nodes {
        if !first.removed_nodes.contains(node) {
            diff.removed_nodes_only_in_second.insert(node.clone());
        }
    }

    // compare storage tries
    for key in first.storage_tries.keys() {
        if second.storage_tries.contains_key(key) {
            let storage_diff = compare_storage_trie_updates(
                first.storage_tries.get(key).unwrap(),
                second.storage_tries.get(key).unwrap(),
            );
            if storage_diff.has_differences() {
                diff.storage_tries_with_differences.insert(*key, storage_diff);
            }
        } else {
            diff.storage_tries_only_in_first.insert(*key);
        }
    }
    for key in second.storage_tries.keys() {
        if !first.storage_tries.contains_key(key) {
            diff.storage_tries_only_in_second.insert(*key);
        }
    }

    diff
}

fn compare_storage_trie_updates(
    first: &StorageTrieUpdates,
    second: &StorageTrieUpdates,
) -> StorageTrieUpdatesDiff {
    let mut diff = StorageTrieUpdatesDiff {
        is_deleted_differs: first.is_deleted != second.is_deleted,
        ..Default::default()
    };

    // compare storage nodes
    for key in first.storage_nodes.keys() {
        if !second.storage_nodes.contains_key(key) {
            diff.storage_nodes_only_in_first.insert(key.clone());
        } else if first.storage_nodes.get(key) != second.storage_nodes.get(key) {
            diff.storage_nodes_with_different_values.insert(key.clone());
        }
    }
    for key in second.storage_nodes.keys() {
        if !first.storage_nodes.contains_key(key) {
            diff.storage_nodes_only_in_second.insert(key.clone());
        }
    }

    // compare removed nodes
    for node in &first.removed_nodes {
        if !second.removed_nodes.contains(node) {
            diff.removed_nodes_only_in_first.insert(node.clone());
        }
    }
    for node in &second.removed_nodes {
        if !first.removed_nodes.contains(node) {
            diff.removed_nodes_only_in_second.insert(node.clone());
        }
    }

    diff
}
