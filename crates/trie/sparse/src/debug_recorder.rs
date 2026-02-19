//! Debug recorder for tracking mutating operations on sparse tries.
//!
//! This module is only available with the `trie-debug` feature and provides
//! infrastructure for recording all mutations to a [`crate::ParallelSparseTrie`]
//! for post-hoc debugging of state root mismatches.

use alloc::{string::String, vec, vec::Vec};
use alloy_primitives::{hex, Bytes, B256};
use alloy_trie::nodes::TrieNode;
use reth_trie_common::Nibbles;
use serde::Serialize;

/// Records mutating operations performed on a sparse trie in the order they occurred.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TrieDebugRecorder {
    initial_state: Vec<InitialNode>,
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

    /// Sets the initial state snapshot of revealed trie nodes.
    ///
    /// This captures all revealed nodes (with their `state_mask` for branches) at a point in time.
    /// For blinded tries this will be empty.
    pub fn set_initial_state(&mut self, state: Vec<InitialNode>) {
        self.initial_state = state;
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
    /// Records a `set_root` call with the root node that was set.
    SetRoot {
        /// The root trie node that was set.
        node: ProofTrieNodeRecord,
    },
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
            crate::LeafUpdate::Changed(value) => Self::Changed(value.clone().into()),
            crate::LeafUpdate::Touched => Self::Touched,
        }
    }
}

/// A serializable snapshot of a single revealed trie node captured as part of the initial state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitialNode {
    /// The nibble path of the node.
    pub path: Nibbles,
    /// The type and relevant data for this node.
    pub node: InitialNodeKind,
    /// The state of the node (dirty or cached).
    pub state: InitialNodeState,
}

/// Serializable mirror of [`crate::SparseNodeState`].
///
/// A separate type is needed because [`crate::SparseNodeState`] contains
/// [`reth_trie_common::RlpNode`] from an external crate that does not implement `Serialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum InitialNodeState {
    /// The node has no state (empty root).
    None,
    /// The node is dirty and its RLP has not been computed.
    Dirty,
    /// The node has a cached RLP.
    Cached {
        /// Hex-encoded RLP node.
        rlp_node: String,
        /// Whether this node is stored in the database trie, if known.
        store_in_db_trie: Option<bool>,
    },
}

impl From<&crate::SparseNodeState> for InitialNodeState {
    fn from(state: &crate::SparseNodeState) -> Self {
        match state {
            crate::SparseNodeState::Dirty => Self::Dirty,
            crate::SparseNodeState::Cached { rlp_node, store_in_db_trie } => Self::Cached {
                rlp_node: hex::encode(rlp_node.as_ref()),
                store_in_db_trie: *store_in_db_trie,
            },
        }
    }
}

/// The kind of a revealed node in the initial state snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum InitialNodeKind {
    /// An empty root node.
    Empty,
    /// A leaf node with its remaining key suffix and value.
    Leaf {
        /// Remaining key suffix.
        key: Nibbles,
        /// The hex-encoded RLP value of the leaf.
        value: String,
    },
    /// An extension node with its key.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
    },
    /// A branch node with its masks and blinded children hashes.
    Branch {
        /// Bitmask of children present.
        state_mask: u16,
        /// Bitmask of children that are blinded.
        blinded_mask: u16,
        /// The hashes of blinded children, as `(nibble, hash)` pairs.
        blinded_hashes: Vec<(u8, B256)>,
    },
}

impl InitialNode {
    /// Creates an [`InitialNode`] from a path, a [`crate::SparseNode`], and an optional leaf value.
    ///
    /// For leaf nodes the `value` should be the RLP-encoded leaf value looked up from the values
    /// map. For non-leaf nodes it is ignored.
    pub fn from_sparse_node(
        path: Nibbles,
        node: &crate::SparseNode,
        value: Option<&Vec<u8>>,
    ) -> Self {
        let (kind, state) = match node {
            crate::SparseNode::Empty => (InitialNodeKind::Empty, InitialNodeState::None),
            crate::SparseNode::Leaf { key, state, .. } => (
                InitialNodeKind::Leaf { key: *key, value: hex::encode(value.unwrap_or(&vec![])) },
                state.into(),
            ),
            crate::SparseNode::Extension { key, state, .. } => {
                (InitialNodeKind::Extension { key: *key }, state.into())
            }
            crate::SparseNode::Branch {
                state_mask, blinded_mask, blinded_hashes, state, ..
            } => {
                let hashes = blinded_mask
                    .iter()
                    .map(|nibble| (nibble, blinded_hashes[nibble as usize]))
                    .collect();
                (
                    InitialNodeKind::Branch {
                        state_mask: state_mask.get(),
                        blinded_mask: blinded_mask.get(),
                        blinded_hashes: hashes,
                    },
                    state.into(),
                )
            }
        };
        Self { path, node: kind, state }
    }
}
