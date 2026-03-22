//! Trie changeset computation.
//!
//! This module provides functionality to compute trie changesets from trie updates.
//! Changesets represent the old values of trie nodes before a block was applied,
//! enabling reorgs by reverting blocks to their previous state.
//!
//! ## Overview
//!
//! When a block is executed, the trie is updated with new node values. To support
//! chain reorganizations, we need to preserve the old values that existed before
//! the block was applied. These old values are called "changesets".
//!
//! ## Usage
//!
//! The primary function is `compute_trie_changesets`, which takes:
//! - A `TrieCursorFactory` for reading current trie state
//! - `TrieUpdatesSorted` containing the new node values
//!
//! And returns `TrieUpdatesSorted` containing the old node values.

use crate::trie_cursor::TrieCursorIter;
use alloy_primitives::{map::B256Map, B256};
use itertools::{merge_join_by, EitherOrBoth};
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
    BranchNodeCompact, Nibbles,
};
use std::cmp::Ordering;

use crate::trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor};

/// Result type for changeset operations.
pub type ChangesetResult<T> = Result<T, DatabaseError>;

/// Computes trie changesets by looking up current node values from the trie.
///
/// Takes the new trie updates and queries the trie for the old values of
/// changed nodes. Returns changesets representing the state before the block
/// was applied, suitable for reorg operations.
///
/// # Arguments
///
/// * `factory` - Trie cursor factory for reading current trie state
/// * `trie_updates` - New trie node values produced by state root computation
///
/// # Returns
///
/// `TrieUpdatesSorted` containing old node values (before this block)
pub fn compute_trie_changesets<Factory>(
    factory: &Factory,
    trie_updates: &TrieUpdatesSorted,
) -> ChangesetResult<TrieUpdatesSorted>
where
    Factory: TrieCursorFactory,
{
    // Compute account trie changesets
    let account_nodes = compute_account_changesets(factory, trie_updates)?;

    // Compute storage trie changesets
    let mut storage_tries = B256Map::default();

    // Create storage cursor once and reuse it for all addresses
    let mut storage_cursor = factory.storage_trie_cursor(B256::default())?;

    for (hashed_address, storage_updates) in trie_updates.storage_tries_ref() {
        storage_cursor.set_hashed_address(*hashed_address);

        let storage_changesets = if storage_updates.is_deleted() {
            // Handle wiped storage
            compute_wiped_storage_changesets(&mut storage_cursor, storage_updates)?
        } else {
            // Handle normal storage updates
            compute_storage_changesets(&mut storage_cursor, storage_updates)?
        };

        if !storage_changesets.is_empty() {
            storage_tries.insert(
                *hashed_address,
                StorageTrieUpdatesSorted {
                    is_deleted: storage_updates.is_deleted(),
                    storage_nodes: storage_changesets,
                },
            );
        }
    }

    // Build and return the result
    Ok(TrieUpdatesSorted::new(account_nodes, storage_tries))
}

/// Computes account trie changesets.
///
/// Looks up the current value for each changed account node path and returns
/// a vector of (path, `old_node`) pairs. The result is already sorted since
/// `trie_updates.account_nodes_ref()` is sorted.
fn compute_account_changesets<Factory>(
    factory: &Factory,
    trie_updates: &TrieUpdatesSorted,
) -> ChangesetResult<Vec<(Nibbles, Option<BranchNodeCompact>)>>
where
    Factory: TrieCursorFactory,
{
    let mut cursor = factory.account_trie_cursor()?;
    let mut account_changesets = Vec::with_capacity(trie_updates.account_nodes_ref().len());

    // For each changed account node, look up its current value
    // The input is already sorted, so the output will be sorted
    for (path, _new_node) in trie_updates.account_nodes_ref() {
        let old_node = cursor.seek_exact(*path)?.map(|(_path, node)| node);
        account_changesets.push((*path, old_node));
    }

    Ok(account_changesets)
}

/// Computes storage trie changesets for a single account.
///
/// Looks up the current value for each changed storage node path and returns
/// a vector of (path, `old_node`) pairs. The result is already sorted since
/// `storage_updates.storage_nodes` is sorted.
///
/// # Arguments
///
/// * `cursor` - Reusable storage trie cursor. The hashed address will be set before use.
/// * `hashed_address` - The hashed address of the account
/// * `storage_updates` - Storage trie updates for this account
fn compute_storage_changesets(
    cursor: &mut impl TrieStorageCursor,
    storage_updates: &StorageTrieUpdatesSorted,
) -> ChangesetResult<Vec<(Nibbles, Option<BranchNodeCompact>)>> {
    let mut storage_changesets = Vec::with_capacity(storage_updates.storage_nodes.len());

    // For each changed storage node, look up its current value
    // The input is already sorted, so the output will be sorted
    for (path, _new_node) in &storage_updates.storage_nodes {
        let old_node = cursor.seek_exact(*path)?.map(|(_path, node)| node);
        storage_changesets.push((*path, old_node));
    }

    Ok(storage_changesets)
}

/// Handles wiped storage trie changeset computation.
///
/// When an account's storage is completely wiped (e.g., account is destroyed),
/// we need to capture not just the changed nodes, but ALL existing nodes in
/// the storage trie, since they all will be deleted.
///
/// This uses an iterator-based approach to avoid allocating an intermediate Vec.
/// It merges two sorted iterators:
/// - Current values of changed paths
/// - All existing nodes in the storage trie
///
/// # Arguments
///
/// * `changed_cursor` - Cursor for looking up changed node values
/// * `wiped_cursor` - Cursor for iterating all nodes in the storage trie
/// * `hashed_address` - The hashed address of the account
/// * `storage_updates` - Storage trie updates for this account
fn compute_wiped_storage_changesets(
    cursor: &mut impl TrieStorageCursor,
    storage_updates: &StorageTrieUpdatesSorted,
) -> ChangesetResult<Vec<(Nibbles, Option<BranchNodeCompact>)>> {
    // Set the hashed address for this account's storage trie
    // Create an iterator that yields all nodes in the storage trie
    let all_nodes = TrieCursorIter::new(cursor);

    // Merge the two sorted iterators
    let merged = storage_trie_wiped_changeset_iter(
        storage_updates.storage_nodes.iter().map(|e| e.0),
        all_nodes,
    )?;

    // Collect into a Vec
    let mut storage_changesets = Vec::new();
    for result in merged {
        storage_changesets.push(result?);
    }

    Ok(storage_changesets)
}

/// Returns an iterator which produces the changeset values for an account whose storage was wiped
/// during a block.
///
/// ## Arguments
///
/// - `curr_values_of_changed` is an iterator over the current values of all trie nodes modified by
///   the block, ordered by path.
/// - `all_nodes` is an iterator over all existing trie nodes for the account, ordered by path.
///
/// ## Returns
///
/// An iterator of trie node paths and a `Some(node)` (indicating the node was wiped) or a `None`
/// (indicating the node was modified in the block but didn't previously exist). The iterator's
/// results will be ordered by path.
pub fn storage_trie_wiped_changeset_iter(
    changed_paths: impl Iterator<Item = Nibbles>,
    all_nodes: impl Iterator<Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
) -> Result<
    impl Iterator<Item = Result<(Nibbles, Option<BranchNodeCompact>), DatabaseError>>,
    DatabaseError,
> {
    let all_nodes = all_nodes.map(|e| e.map(|(nibbles, node)| (nibbles, Some(node))));

    let merged = merge_join_by(changed_paths, all_nodes, |a, b| match (a, b) {
        (_, Err(_)) => Ordering::Greater,
        (a, Ok(b)) => a.cmp(&b.0),
    });

    Ok(merged.map(|either_or| match either_or {
        EitherOrBoth::Left(changed) => {
            // A path of a changed node which was not found in the database. The current value of
            // this path must be None, otherwise it would have also been returned by the `all_nodes`
            // iter.
            Ok((changed, None))
        }
        EitherOrBoth::Right(wiped) => {
            // A node was found in the db (indicating it was wiped) but was not a changed node.
            // Return it as-is.
            wiped
        }
        EitherOrBoth::Both(_changed, wiped) => {
            // A path of a changed node was found with a previous value in the database. If the
            // changed node had no previous value (None) it wouldn't be returned by `all_nodes` and
            // so would be in the Left branch.
            //
            // Due to the ordering closure passed to `merge_join_by` it's not possible for wrapped
            // to be an error here.
            debug_assert!(wiped.is_ok(), "unreachable error condition: {wiped:?}");
            wiped
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::mock::MockTrieCursorFactory;
    use alloy_primitives::map::B256Map;
    use reth_trie_common::updates::StorageTrieUpdatesSorted;
    use std::collections::BTreeMap;

    #[test]
    fn test_empty_updates() {
        // Create an empty mock factory
        // Note: We need to include B256::default() in storage_tries because
        // compute_trie_changesets creates cursors for it upfront
        let mut storage_tries = B256Map::default();
        storage_tries.insert(B256::default(), BTreeMap::new());
        let factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_tries);

        // Create empty updates
        let updates = TrieUpdatesSorted::new(vec![], B256Map::default());

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Should produce empty changesets
        assert!(changesets.account_nodes_ref().is_empty());
        assert!(changesets.storage_tries_ref().is_empty());
    }

    #[test]
    fn test_account_changesets() {
        // Create some initial account trie state
        let path1 = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let path2 = Nibbles::from_nibbles([0x4, 0x5, 0x6]);
        // tree_mask and hash_mask must be subsets of state_mask
        let node1 = BranchNodeCompact::new(0b1111, 0b1010, 0, vec![], None);
        let node2 = BranchNodeCompact::new(0b1111, 0b1100, 0, vec![], None);

        let mut account_nodes = BTreeMap::new();
        account_nodes.insert(path1, node1.clone());
        account_nodes.insert(path2, node2);

        // Need to include B256::default() for cursor creation
        let mut storage_tries = B256Map::default();
        storage_tries.insert(B256::default(), BTreeMap::new());
        let factory = MockTrieCursorFactory::new(account_nodes, storage_tries);

        // Create updates that modify path1 and add a new path3
        let path3 = Nibbles::from_nibbles([0x7, 0x8, 0x9]);
        let new_node1 = BranchNodeCompact::new(0b1111, 0b0001, 0, vec![], None);
        let new_node3 = BranchNodeCompact::new(0b1111, 0b0000, 0, vec![], None);

        let updates = TrieUpdatesSorted::new(
            vec![(path1, Some(new_node1)), (path3, Some(new_node3))],
            B256Map::default(),
        );

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Check account changesets
        assert_eq!(changesets.account_nodes_ref().len(), 2);

        // path1 should have the old node1 value
        assert_eq!(changesets.account_nodes_ref()[0].0, path1);
        assert_eq!(changesets.account_nodes_ref()[0].1, Some(node1));

        // path3 should have None (it didn't exist before)
        assert_eq!(changesets.account_nodes_ref()[1].0, path3);
        assert_eq!(changesets.account_nodes_ref()[1].1, None);
    }

    #[test]
    fn test_storage_changesets() {
        let hashed_address = B256::from([1u8; 32]);

        // Create some initial storage trie state
        let path1 = Nibbles::from_nibbles([0x1, 0x2]);
        let path2 = Nibbles::from_nibbles([0x3, 0x4]);
        let node1 = BranchNodeCompact::new(0b1111, 0b0011, 0, vec![], None);
        let node2 = BranchNodeCompact::new(0b1111, 0b0101, 0, vec![], None);

        let mut storage_nodes = BTreeMap::new();
        storage_nodes.insert(path1, node1.clone());
        storage_nodes.insert(path2, node2);

        let mut storage_tries = B256Map::default();
        storage_tries.insert(B256::default(), BTreeMap::new()); // For cursor creation
        storage_tries.insert(hashed_address, storage_nodes);

        let factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_tries);

        // Create updates that modify path1 and add a new path3
        let path3 = Nibbles::from_nibbles([0x5, 0x6]);
        let new_node1 = BranchNodeCompact::new(0b1111, 0b1000, 0, vec![], None);
        let new_node3 = BranchNodeCompact::new(0b1111, 0b0000, 0, vec![], None);

        let mut storage_updates = B256Map::default();
        storage_updates.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: false,
                storage_nodes: vec![(path1, Some(new_node1)), (path3, Some(new_node3))],
            },
        );

        let updates = TrieUpdatesSorted::new(vec![], storage_updates);

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Check storage changesets
        assert_eq!(changesets.storage_tries_ref().len(), 1);
        let storage_changesets = changesets.storage_tries_ref().get(&hashed_address).unwrap();
        assert!(!storage_changesets.is_deleted);
        assert_eq!(storage_changesets.storage_nodes.len(), 2);

        // path1 should have the old node1 value
        assert_eq!(storage_changesets.storage_nodes[0].0, path1);
        assert_eq!(storage_changesets.storage_nodes[0].1, Some(node1));

        // path3 should have None (it didn't exist before)
        assert_eq!(storage_changesets.storage_nodes[1].0, path3);
        assert_eq!(storage_changesets.storage_nodes[1].1, None);
    }

    #[test]
    fn test_wiped_storage() {
        let hashed_address = B256::from([2u8; 32]);

        // Create initial storage trie with multiple nodes
        let path1 = Nibbles::from_nibbles([0x1, 0x2]);
        let path2 = Nibbles::from_nibbles([0x3, 0x4]);
        let path3 = Nibbles::from_nibbles([0x5, 0x6]);
        let node1 = BranchNodeCompact::new(0b1111, 0b0011, 0, vec![], None);
        let node2 = BranchNodeCompact::new(0b1111, 0b0101, 0, vec![], None);
        let node3 = BranchNodeCompact::new(0b1111, 0b1001, 0, vec![], None);

        let mut storage_nodes = BTreeMap::new();
        storage_nodes.insert(path1, node1.clone());
        storage_nodes.insert(path2, node2.clone());
        storage_nodes.insert(path3, node3.clone());

        let mut storage_tries = B256Map::default();
        storage_tries.insert(B256::default(), BTreeMap::new()); // For cursor creation
        storage_tries.insert(hashed_address, storage_nodes);

        let factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_tries);

        // Create updates that modify path1 and mark storage as wiped
        let new_node1 = BranchNodeCompact::new(0b1111, 0b1000, 0, vec![], None);

        let mut storage_updates = B256Map::default();
        storage_updates.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: true,
                storage_nodes: vec![(path1, Some(new_node1))],
            },
        );

        let updates = TrieUpdatesSorted::new(vec![], storage_updates);

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Check storage changesets
        assert_eq!(changesets.storage_tries_ref().len(), 1);
        let storage_changesets = changesets.storage_tries_ref().get(&hashed_address).unwrap();
        assert!(storage_changesets.is_deleted);

        // Should include all three nodes (changed path1 + wiped path2 and path3)
        assert_eq!(storage_changesets.storage_nodes.len(), 3);

        // All paths should be present in sorted order
        assert_eq!(storage_changesets.storage_nodes[0].0, path1);
        assert_eq!(storage_changesets.storage_nodes[1].0, path2);
        assert_eq!(storage_changesets.storage_nodes[2].0, path3);

        // All should have their old values
        assert_eq!(storage_changesets.storage_nodes[0].1, Some(node1));
        assert_eq!(storage_changesets.storage_nodes[1].1, Some(node2));
        assert_eq!(storage_changesets.storage_nodes[2].1, Some(node3));
    }

    #[test]
    fn test_wiped_storage_with_new_path() {
        let hashed_address = B256::from([3u8; 32]);

        // Create initial storage trie with two nodes
        let path1 = Nibbles::from_nibbles([0x1, 0x2]);
        let path3 = Nibbles::from_nibbles([0x5, 0x6]);
        let node1 = BranchNodeCompact::new(0b1111, 0b0011, 0, vec![], None);
        let node3 = BranchNodeCompact::new(0b1111, 0b1001, 0, vec![], None);

        let mut storage_nodes = BTreeMap::new();
        storage_nodes.insert(path1, node1.clone());
        storage_nodes.insert(path3, node3.clone());

        let mut storage_tries = B256Map::default();
        storage_tries.insert(B256::default(), BTreeMap::new()); // For cursor creation
        storage_tries.insert(hashed_address, storage_nodes);

        let factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_tries);

        // Create updates that add a new path2 that didn't exist before
        let path2 = Nibbles::from_nibbles([0x3, 0x4]);
        let new_node2 = BranchNodeCompact::new(0b1111, 0b0101, 0, vec![], None);

        let mut storage_updates = B256Map::default();
        storage_updates.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: true,
                storage_nodes: vec![(path2, Some(new_node2))],
            },
        );

        let updates = TrieUpdatesSorted::new(vec![], storage_updates);

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Check storage changesets
        let storage_changesets = changesets.storage_tries_ref().get(&hashed_address).unwrap();
        assert!(storage_changesets.is_deleted);

        // Should include all three paths: existing path1, new path2, existing path3
        assert_eq!(storage_changesets.storage_nodes.len(), 3);

        // Check sorted order
        assert_eq!(storage_changesets.storage_nodes[0].0, path1);
        assert_eq!(storage_changesets.storage_nodes[1].0, path2);
        assert_eq!(storage_changesets.storage_nodes[2].0, path3);

        // path1 and path3 have old values, path2 has None (didn't exist)
        assert_eq!(storage_changesets.storage_nodes[0].1, Some(node1));
        assert_eq!(storage_changesets.storage_nodes[1].1, None);
        assert_eq!(storage_changesets.storage_nodes[2].1, Some(node3));
    }
}
