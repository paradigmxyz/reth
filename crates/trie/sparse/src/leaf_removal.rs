//! Shared helper functions for leaf removal operations in sparse tries.
//!
//! These pure functions compute structural transformations needed when removing leaves
//! from a sparse trie. They are used by both `SerialSparseTrie` and `ParallelSparseTrie`.

use crate::SparseNode;
use reth_trie_common::{Nibbles, TrieMask};

/// Computes the replacement node for a branch when it has only one remaining child after
/// a leaf removal.
///
/// Given a parent branch node's path and the remaining child's path and node, this function
/// determines what the branch should be replaced with:
/// - If the child is a leaf: collapse into a leaf with prepended nibble
/// - If the child is an extension: collapse into a longer extension
/// - If the child is a branch: downgrade to a one-nibble extension
///
/// # Returns
///
/// A tuple of:
/// - The replacement node for the branch
/// - `true` if the child node should be removed, `false` if it should be kept
///
/// # Panics
///
/// - If `remaining_child_node` is `Empty` or `Hash` (must be revealed)
/// - If `remaining_child_path` doesn't start with `parent_path`
pub fn branch_changes_on_leaf_removal(
    parent_path: &Nibbles,
    remaining_child_path: &Nibbles,
    remaining_child_node: &SparseNode,
) -> (SparseNode, bool) {
    debug_assert!(remaining_child_path.len() > parent_path.len());
    debug_assert!(remaining_child_path.starts_with(parent_path));

    let remaining_child_nibble = remaining_child_path.get_unchecked(parent_path.len());

    match remaining_child_node {
        SparseNode::Empty | SparseNode::Hash(_) => {
            panic!("remaining child must have been revealed already")
        }
        // If the only child is a leaf node, we downgrade the branch node into a
        // leaf node, prepending the nibble to the key, and delete the old child.
        SparseNode::Leaf { key, .. } => {
            let mut new_key = Nibbles::from_nibbles_unchecked([remaining_child_nibble]);
            new_key.extend(key);
            (SparseNode::new_leaf(new_key), true)
        }
        // If the only child node is an extension node, we downgrade the branch
        // node into an even longer extension node, prepending the nibble to the
        // key, and delete the old child.
        SparseNode::Extension { key, .. } => {
            let mut new_key = Nibbles::from_nibbles_unchecked([remaining_child_nibble]);
            new_key.extend(key);
            (SparseNode::new_ext(new_key), true)
        }
        // If the only child is a branch node, we downgrade the current branch
        // node into a one-nibble extension node.
        SparseNode::Branch { .. } => (
            SparseNode::new_ext(Nibbles::from_nibbles_unchecked([remaining_child_nibble])),
            false,
        ),
    }
}

/// Computes an optional replacement for an extension node when its child has been
/// modified during leaf removal.
///
/// After collapsing a branch into a leaf or extension, the parent extension may need
/// to be merged with its new child:
/// - If the child is now a leaf: merge into a single leaf
/// - If the child is now an extension: merge into a single longer extension
/// - If the child is still a branch: no change needed
///
/// # Returns
///
/// - `Some(node)` if the extension should be replaced (and the child deleted)
/// - `None` if no change is needed
///
/// # Panics
///
/// - If `child` is `Empty` or `Hash` (must be revealed)
/// - If `child_path` doesn't start with `parent_path`
pub fn extension_changes_on_leaf_removal(
    parent_path: &Nibbles,
    parent_key: &Nibbles,
    child_path: &Nibbles,
    child: &SparseNode,
) -> Option<SparseNode> {
    debug_assert!(child_path.len() > parent_path.len());
    debug_assert!(child_path.starts_with(parent_path));

    match child {
        SparseNode::Empty | SparseNode::Hash(_) => {
            panic!("child must be revealed")
        }
        // For a leaf node, we collapse the extension node into a leaf node,
        // extending the key. While it's impossible to encounter an extension node
        // followed by a leaf node in a complete trie, it's possible here because we
        // could have downgraded the extension node's child into a leaf node from a
        // branch in a previous call to `branch_changes_on_leaf_removal`.
        SparseNode::Leaf { key, .. } => {
            let mut new_key = *parent_key;
            new_key.extend(key);
            Some(SparseNode::new_leaf(new_key))
        }
        // Similar to the leaf node, for an extension node, we collapse them into one
        // extension node, extending the key.
        SparseNode::Extension { key, .. } => {
            let mut new_key = *parent_key;
            new_key.extend(key);
            Some(SparseNode::new_ext(new_key))
        }
        // For a branch node, we just leave the extension node as-is.
        SparseNode::Branch { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_changes_leaf_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2]);
        let child_node = SparseNode::new_leaf(Nibbles::from_nibbles([0x3, 0x4]));

        let (new_node, delete_child) =
            branch_changes_on_leaf_removal(&parent_path, &child_path, &child_node);

        assert!(delete_child);
        match new_node {
            SparseNode::Leaf { key, .. } => {
                assert_eq!(key, Nibbles::from_nibbles([0x2, 0x3, 0x4]));
            }
            _ => panic!("expected leaf node"),
        }
    }

    #[test]
    fn test_branch_changes_extension_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2]);
        let child_node = SparseNode::new_ext(Nibbles::from_nibbles([0x3, 0x4]));

        let (new_node, delete_child) =
            branch_changes_on_leaf_removal(&parent_path, &child_path, &child_node);

        assert!(delete_child);
        match new_node {
            SparseNode::Extension { key, .. } => {
                assert_eq!(key, Nibbles::from_nibbles([0x2, 0x3, 0x4]));
            }
            _ => panic!("expected extension node"),
        }
    }

    #[test]
    fn test_branch_changes_branch_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2]);
        let child_node = SparseNode::new_branch(TrieMask::new(0b11));

        let (new_node, delete_child) =
            branch_changes_on_leaf_removal(&parent_path, &child_path, &child_node);

        assert!(!delete_child);
        match new_node {
            SparseNode::Extension { key, .. } => {
                assert_eq!(key, Nibbles::from_nibbles([0x2]));
            }
            _ => panic!("expected extension node"),
        }
    }

    #[test]
    fn test_extension_changes_leaf_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let parent_key = Nibbles::from_nibbles([0x2, 0x3]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let child_node = SparseNode::new_leaf(Nibbles::from_nibbles([0x4, 0x5]));

        let result =
            extension_changes_on_leaf_removal(&parent_path, &parent_key, &child_path, &child_node);

        match result {
            Some(SparseNode::Leaf { key, .. }) => {
                assert_eq!(key, Nibbles::from_nibbles([0x2, 0x3, 0x4, 0x5]));
            }
            _ => panic!("expected Some(leaf)"),
        }
    }

    #[test]
    fn test_extension_changes_extension_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let parent_key = Nibbles::from_nibbles([0x2, 0x3]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let child_node = SparseNode::new_ext(Nibbles::from_nibbles([0x4, 0x5]));

        let result =
            extension_changes_on_leaf_removal(&parent_path, &parent_key, &child_path, &child_node);

        match result {
            Some(SparseNode::Extension { key, .. }) => {
                assert_eq!(key, Nibbles::from_nibbles([0x2, 0x3, 0x4, 0x5]));
            }
            _ => panic!("expected Some(extension)"),
        }
    }

    #[test]
    fn test_extension_changes_branch_child() {
        let parent_path = Nibbles::from_nibbles([0x1]);
        let parent_key = Nibbles::from_nibbles([0x2, 0x3]);
        let child_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let child_node = SparseNode::new_branch(TrieMask::new(0b11));

        let result =
            extension_changes_on_leaf_removal(&parent_path, &parent_key, &child_path, &child_node);

        assert!(result.is_none());
    }
}
