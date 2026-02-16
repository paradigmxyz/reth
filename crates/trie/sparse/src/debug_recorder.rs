//! Debug recorder for tracking mutating operations on sparse tries.
//!
//! This module is only available with the `trie-debug` feature and provides
//! infrastructure for recording all mutations to a [`crate::ParallelSparseTrie`]
//! for post-hoc debugging of state root mismatches.

use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256};
use reth_trie_common::Nibbles;
use serde::{Deserialize, Serialize};

/// Records mutating operations performed on a sparse trie in the order they occurred.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrieDebugRecorder {
    ops: Vec<RecordedOp>,
}

impl TrieDebugRecorder {
    /// Creates a new empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all recorded operations.
    pub fn reset(&mut self) {
        self.ops.clear();
    }

    /// Records a single operation.
    pub fn record(&mut self, op: RecordedOp) {
        self.ops.push(op);
    }

    /// Returns a reference to the recorded operations.
    pub fn ops(&self) -> &[RecordedOp] {
        &self.ops
    }

    /// Takes and returns the recorded operations, leaving the recorder empty.
    pub fn take_ops(&mut self) -> Vec<RecordedOp> {
        core::mem::take(&mut self.ops)
    }

    /// Returns `true` if no operations have been recorded.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// A mutating operation recorded from a sparse trie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecordedOp {
    /// Records a `reveal_nodes` call with the nodes that were revealed.
    RevealNodes {
        /// The proof trie nodes that were revealed.
        nodes: Vec<ProofTrieNodeRecord>,
    },
    /// Records an `update_leaves` call with the leaf updates.
    UpdateLeaves {
        /// The leaf updates that were applied.
        updates: Vec<(B256, LeafUpdateRecord)>,
    },
    /// Records an `update_subtrie_hashes` call.
    UpdateSubtrieHashes,
    /// Records a `root()` call.
    Root,
}

/// A serializable record of a proof trie node.
///
/// This is a simplified version of [`reth_trie_common::ProofTrieNode`] that stores
/// the RLP-encoded node bytes rather than the full [`alloy_trie::nodes::TrieNode`]
/// (which does not implement `Serialize`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTrieNodeRecord {
    /// The nibble path of the node.
    pub path: Nibbles,
    /// The RLP-encoded trie node.
    pub node_rlp: Bytes,
    /// The branch node masks `(hash_mask, tree_mask)` stored as raw `u16` values, if present.
    pub masks: Option<(u16, u16)>,
}

impl ProofTrieNodeRecord {
    /// Creates a record from a [`reth_trie_common::ProofTrieNode`].
    pub fn from_proof_trie_node(node: &reth_trie_common::ProofTrieNode) -> Self {
        use alloy_rlp::Encodable;
        let mut node_rlp = Vec::new();
        node.node.encode(&mut node_rlp);
        Self {
            path: node.path,
            node_rlp: node_rlp.into(),
            masks: node.masks.map(|masks| (masks.hash_mask.get(), masks.tree_mask.get())),
        }
    }
}

/// A serializable record of a leaf update, mirroring [`crate::LeafUpdate`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LeafUpdateRecord {
    /// The leaf value was changed to the given RLP-encoded value.
    Changed(Bytes),
    /// The leaf value was touched but the new value is not yet known.
    Touched,
}

impl From<&crate::LeafUpdate> for LeafUpdateRecord {
    fn from(update: &crate::LeafUpdate) -> Self {
        match update {
            crate::LeafUpdate::Changed(value) => Self::Changed(value.clone().into()),
            crate::LeafUpdate::Touched => Self::Touched,
        }
    }
}
