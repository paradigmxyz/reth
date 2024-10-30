#![allow(missing_docs)]

use reth_trie::{BranchNodeCompact, Nibbles};
use std::collections::{HashMap, HashSet};

/// Helper function to verify tree mask invariant similar to the fuzzer
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
