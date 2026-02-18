//! Debug recorder for tracking mutating operations on sparse tries.
//!
//! This module is only available with the `trie-debug` feature and provides
//! infrastructure for recording all mutations to a [`crate::ParallelSparseTrie`]
//! for post-hoc debugging of state root mismatches.

use alloc::{string::String, vec::Vec};
use alloy_primitives::{hex, Bytes, B256};
use alloy_trie::nodes::TrieNode;
use reth_trie_common::Nibbles;
use serde::Serialize;

/// Records mutating operations performed on a sparse trie in the order they occurred.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
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
    pub const fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// A mutating operation recorded from a sparse trie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        /// Keys remaining in the updates map after the call (i.e. those that could not be applied
        /// due to blinded nodes).
        remaining_keys: Vec<B256>,
        /// Proof targets returned via the callback as `(key, min_len)` pairs.
        proof_targets: Vec<(B256, u8)>,
    },
    /// Records an `update_subtrie_hashes` call.
    UpdateSubtrieHashes,
    /// Records a `root()` call.
    Root,
}

/// A serializable record of a proof trie node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProofTrieNodeRecord {
    /// The nibble path of the node.
    pub path: Nibbles,
    /// The trie node.
    pub node: TrieNodeRecord,
    /// The branch node masks `(hash_mask, tree_mask)` stored as raw `u16` values, if present.
    pub masks: Option<(u16, u16)>,
}

impl ProofTrieNodeRecord {
    /// Creates a record from a [`reth_trie_common::ProofTrieNode`].
    pub fn from_proof_trie_node(node: &reth_trie_common::ProofTrieNode) -> Self {
        Self {
            path: node.path,
            node: TrieNodeRecord(node.node.clone()),
            masks: node.masks.map(|masks| (masks.hash_mask.get(), masks.tree_mask.get())),
        }
    }

    /// Creates a record from a [`reth_trie_common::ProofTrieNodeV2`].
    pub fn from_proof_trie_node_v2(node: &reth_trie_common::ProofTrieNodeV2) -> Self {
        use reth_trie_common::TrieNodeV2;
        let trie_node = match &node.node {
            TrieNodeV2::EmptyRoot => TrieNode::EmptyRoot,
            TrieNodeV2::Leaf(leaf) => TrieNode::Leaf(leaf.clone()),
            TrieNodeV2::Extension(ext) => TrieNode::Extension(ext.clone()),
            TrieNodeV2::Branch(branch) => TrieNode::Branch(alloy_trie::nodes::BranchNode::new(
                branch.stack.clone(),
                branch.state_mask,
            )),
        };
        Self {
            path: node.path,
            node: TrieNodeRecord(trie_node),
            masks: node.masks.map(|masks| (masks.hash_mask.get(), masks.tree_mask.get())),
        }
    }
}

/// A newtype wrapper around [`TrieNode`] with custom serialization that hex-encodes byte fields
/// (leaf values, branch stack entries, extension child pointers) instead of serializing them as
/// raw integer arrays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrieNodeRecord(pub TrieNode);

impl From<TrieNode> for TrieNodeRecord {
    fn from(node: TrieNode) -> Self {
        Self(node)
    }
}

impl Serialize for TrieNodeRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStructVariant;
        match &self.0 {
            TrieNode::EmptyRoot => serializer.serialize_unit_variant("TrieNode", 0, "EmptyRoot"),
            TrieNode::Branch(branch) => {
                let stack_hex: Vec<String> =
                    branch.stack.iter().map(|n| hex::encode(n.as_ref())).collect();
                let mut sv = serializer.serialize_struct_variant("TrieNode", 1, "Branch", 2)?;
                sv.serialize_field("stack", &stack_hex)?;
                sv.serialize_field("state_mask", &branch.state_mask.get())?;
                sv.end()
            }
            TrieNode::Extension(ext) => {
                let mut sv = serializer.serialize_struct_variant("TrieNode", 2, "Extension", 2)?;
                sv.serialize_field("key", &ext.key)?;
                sv.serialize_field("child", &hex::encode(ext.child.as_ref()))?;
                sv.end()
            }
            TrieNode::Leaf(leaf) => {
                let mut sv = serializer.serialize_struct_variant("TrieNode", 3, "Leaf", 2)?;
                sv.serialize_field("key", &leaf.key)?;
                sv.serialize_field("value", &hex::encode(&leaf.value))?;
                sv.end()
            }
        }
    }
}

/// A serializable record of a leaf update, mirroring [`crate::LeafUpdate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LeafUpdateRecord {
    /// The leaf value was changed to the given RLP-encoded value.
    Changed(Bytes),
    /// The leaf value was touched but the new value is not yet known.
    Touched,
}

impl From<&crate::LeafUpdate> for LeafUpdateRecord {
    fn from(update: &crate::LeafUpdate) -> Self {
        match update {
            crate::LeafUpdate::Changed(value) => Self::Changed(value.to_vec().into()),
            crate::LeafUpdate::Touched => Self::Touched,
        }
    }
}
