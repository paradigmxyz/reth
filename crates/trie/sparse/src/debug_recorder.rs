//! Debug recorder for tracking mutating operations on sparse tries.
//!
//! This module is only available with the `trie-debug` feature and provides
//! infrastructure for recording all mutations to a [`crate::ArenaParallelSparseTrie`]
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
        /// Proof targets returned via the callback as `(key, min_len)` pairs.
        proof_targets: Vec<(B256, u8)>,
    },
    /// Records an `update_subtrie_hashes` call.
    UpdateSubtrieHashes,
    /// Records a `root()` call.
    Root,
    /// Records a `set_root` call with the root node that was set.
    SetRoot {
        /// The root trie node that was set.
        node: ProofTrieNodeRecord,
    },
    /// Records a `prune` call. Emitted before the post-prune `SetRoot`/`RevealNodes`
    /// so consumers know the following initial state is the result of pruning.
    Prune,
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
    /// The short key (extension key) of a branch node. When present, the branch's logical path
    /// is `path` extended by this key. The arena trie merges extension nodes into their child
    /// branch, so this replaces separate `Extension` node records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_key: Option<Nibbles>,
    /// The node's state (`Revealed`, `Cached`, or `Dirty`), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeStateRecord>,
}

/// A serializable record of a node's state, mirroring the arena trie's `ArenaSparseNodeState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum NodeStateRecord {
    /// The node has been revealed but its RLP encoding is not cached.
    Revealed,
    /// The node has a cached RLP encoding that is still valid.
    Cached {
        /// The cached RLP-encoded node, hex-encoded.
        rlp_node: String,
    },
    /// The node has been modified and its RLP encoding needs recomputation.
    Dirty,
}

impl ProofTrieNodeRecord {
    /// Creates a record from a [`reth_trie_common::ProofTrieNode`].
    pub fn from_proof_trie_node(node: &reth_trie_common::ProofTrieNode) -> Self {
        Self {
            path: node.path,
            node: TrieNodeRecord(node.node.clone()),
            masks: node.masks.map(|masks| (masks.hash_mask.get(), masks.tree_mask.get())),
            short_key: None,
            state: None,
        }
    }

    /// Creates a record from a [`reth_trie_common::ProofTrieNodeV2`].
    pub fn from_proof_trie_node_v2(node: &reth_trie_common::ProofTrieNodeV2) -> Self {
        use reth_trie_common::TrieNodeV2;
        let (trie_node, short_key) = match &node.node {
            TrieNodeV2::EmptyRoot => (TrieNode::EmptyRoot, None),
            TrieNodeV2::Leaf(leaf) => (TrieNode::Leaf(leaf.clone()), None),
            TrieNodeV2::Extension(ext) => (TrieNode::Extension(ext.clone()), None),
            TrieNodeV2::Branch(branch) => (
                TrieNode::Branch(alloy_trie::nodes::BranchNode::new(
                    branch.stack.clone(),
                    branch.state_mask,
                )),
                (!branch.key.is_empty()).then_some(branch.key),
            ),
        };
        Self {
            path: node.path,
            node: TrieNodeRecord(trie_node),
            masks: node.masks.map(|masks| (masks.hash_mask.get(), masks.tree_mask.get())),
            short_key,
            state: None,
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
            crate::LeafUpdate::Changed(value) => Self::Changed(value.clone().into()),
            crate::LeafUpdate::Touched => Self::Touched,
        }
    }
}
