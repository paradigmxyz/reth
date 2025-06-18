use alloc::{vec, vec::Vec};
use alloy_trie::Nibbles;
use reth_trie_common::RlpNode;
use smallvec::SmallVec;

use crate::SparseNodeType;

/// Collection of reusable buffers for [`crate::RevealedSparseTrie::rlp_node`] calculations.
///
/// These buffers reduce allocations when computing RLP representations during trie updates.
#[derive(Debug, Default)]
pub struct RlpNodeBuffers {
    /// Stack of RLP node paths
    pub path_stack: Vec<RlpNodePathStackItem>,
    /// Stack of RLP nodes
    pub rlp_node_stack: Vec<RlpNodeStackItem>,
    /// Reusable branch child path
    pub branch_child_buf: SmallVec<[Nibbles; 16]>,
    /// Reusable branch value stack
    pub branch_value_stack_buf: SmallVec<[RlpNode; 16]>,
}

impl RlpNodeBuffers {
    /// Creates a new instance of buffers with the root path on the stack.
    pub fn new_with_root_path() -> Self {
        Self {
            path_stack: vec![RlpNodePathStackItem {
                level: 0,
                path: Nibbles::default(),
                is_in_prefix_set: None,
            }],
            rlp_node_stack: Vec::new(),
            branch_child_buf: SmallVec::<[Nibbles; 16]>::new_const(),
            branch_value_stack_buf: SmallVec::<[RlpNode; 16]>::new_const(),
        }
    }
}

/// RLP node path stack item.
#[derive(Debug)]
pub struct RlpNodePathStackItem {
    /// Level at which the node is located. Higher numbers correspond to lower levels in the trie.
    pub level: usize,
    /// Path to the node.
    pub path: Nibbles,
    /// Whether the path is in the prefix set. If [`None`], then unknown yet.
    pub is_in_prefix_set: Option<bool>,
}

/// RLP node stack item.
#[derive(Debug)]
pub struct RlpNodeStackItem {
    /// Path to the node.
    pub path: Nibbles,
    /// RLP node.
    pub rlp_node: RlpNode,
    /// Type of the node.
    pub node_type: SparseNodeType,
}
