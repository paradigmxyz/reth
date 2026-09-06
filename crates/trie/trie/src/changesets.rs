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

use crate::trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use alloy_primitives::{map::B256Map, B256};
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
    BranchNodeCompact, Nibbles,
};

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

        let storage_changesets = compute_storage_changesets(&mut storage_cursor, storage_updates)?;

        if !storage_changesets.is_empty() {
            storage_tries.insert(
                *hashed_address,
                StorageTrieUpdatesSorted { storage_nodes: storage_changesets },
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
                storage_nodes: vec![(path1, Some(new_node1)), (path3, Some(new_node3))],
            },
        );

        let updates = TrieUpdatesSorted::new(vec![], storage_updates);

        // Compute changesets
        let changesets = compute_trie_changesets(&factory, &updates).unwrap();

        // Check storage changesets
        assert_eq!(changesets.storage_tries_ref().len(), 1);
        let storage_changesets = changesets.storage_tries_ref().get(&hashed_address).unwrap();
        assert_eq!(storage_changesets.storage_nodes.len(), 2);

        // path1 should have the old node1 value
        assert_eq!(storage_changesets.storage_nodes[0].0, path1);
        assert_eq!(storage_changesets.storage_nodes[0].1, Some(node1));

        // path3 should have None (it didn't exist before)
        assert_eq!(storage_changesets.storage_nodes[1].0, path3);
        assert_eq!(storage_changesets.storage_nodes[1].1, None);
    }
}
