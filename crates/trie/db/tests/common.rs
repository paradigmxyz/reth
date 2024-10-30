#![allow(missing_docs)]

use reth_trie::{BranchNodeCompact, Nibbles};
use std::collections::{HashMap, HashSet};

/// Helper function to verify tree mask invariant: for each branch node in trie
/// nodes with path, for each position with 1 in its tree mask, there must be
/// branch nodes in trie updates at paths path-position.
///
/// For instance, for branch node at path 0x01 with tree mask 0100000001010101,
/// in trie updates we must have branch nodes at paths 0x010, 0x012, 0x014,
/// 0x016, 0x01e (the mask indicates child nibbles but it goes from the right to
/// left).
#[allow(dead_code)]
pub(crate) fn verify_tree_mask_invariant(
    nodes: &HashMap<Nibbles, BranchNodeCompact>,
    removed: &HashSet<Nibbles>,
) -> bool {
    for (path, branch_node) in nodes {
        let child_paths = (0..16)
            .filter(|&pos| (branch_node.tree_mask.get() & (1 << pos)) != 0)
            .map(|pos| {
                let mut child_path = path.clone();
                child_path.push(pos as u8);
                child_path
            })
            .collect::<Vec<_>>();

        for child_path in child_paths {
            if !nodes.contains_key(&child_path) && !removed.contains(&child_path) {
                println!(
                    "Missing child node at path: {:?} for parent: {:?} with mask: {:016b}",
                    child_path,
                    path,
                    branch_node.tree_mask.get()
                );
                return false;
            }
        }
    }
    true
}
