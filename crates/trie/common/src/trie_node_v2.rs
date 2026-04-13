//! Version 2 types related to representing nodes in an MPT.

use crate::BranchNodeMasks;
use alloc::vec::Vec;
use alloy_primitives::hex;
use alloy_rlp::{bytes, Decodable, Encodable, EMPTY_STRING_CODE};
use alloy_trie::{
    nodes::{BranchNodeRef, ExtensionNode, ExtensionNodeRef, LeafNode, RlpNode, TrieNode},
    Nibbles, TrieMask,
};
use core::fmt;

/// Carries all information needed by a sparse trie to reveal a particular node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofTrieNodeV2 {
    /// Path of the node.
    pub path: Nibbles,
    /// The node itself.
    pub node: TrieNodeV2,
    /// Tree and hash masks for the node, if known.
    /// Both masks are always set together (from database branch nodes).
    pub masks: Option<BranchNodeMasks>,
}

impl ProofTrieNodeV2 {
    /// Creates an empty `ProofTrieNodeV2` with an empty root node. Useful as a placeholder when
    /// taking a node out of a slice via [`core::mem::replace`].
    pub fn empty() -> Self {
        Self { path: Nibbles::default(), node: TrieNodeV2::EmptyRoot, masks: None }
    }

    /// Converts an iterator of `(path, TrieNode, masks)` tuples into `Vec<ProofTrieNodeV2>`,
    /// merging extension nodes into their child branch nodes.
    ///
    /// The input **must** be sorted in depth-first order (children before parents) for extension
    /// merging to work correctly.
    pub fn from_sorted_trie_nodes(
        iter: impl IntoIterator<Item = (Nibbles, TrieNode, Option<BranchNodeMasks>)>,
    ) -> Vec<Self> {
        let iter = iter.into_iter();
        let mut result = Vec::with_capacity(iter.size_hint().0);

        for (path, node, masks) in iter {
            match node {
                TrieNode::EmptyRoot => {
                    result.push(Self { path, node: TrieNodeV2::EmptyRoot, masks });
                }
                TrieNode::Leaf(leaf) => {
                    result.push(Self { path, node: TrieNodeV2::Leaf(leaf), masks });
                }
                TrieNode::Branch(branch) => {
                    result.push(Self {
                        path,
                        node: TrieNodeV2::Branch(BranchNodeV2 {
                            key: Nibbles::new(),
                            branch_rlp_node: None,
                            stack: branch.stack,
                            state_mask: branch.state_mask,
                        }),
                        masks,
                    });
                }
                TrieNode::Extension(ext) => {
                    // In depth-first order, the child branch comes BEFORE the parent
                    // extension. The child branch should be the last item we added to
                    // result, at path extension.path + extension.key.
                    let expected_branch_path = path.join(&ext.key);

                    // Check if the last item in result is the child branch
                    if let Some(last) = result.last_mut() &&
                        last.path == expected_branch_path &&
                        let TrieNodeV2::Branch(branch_v2) = &mut last.node
                    {
                        debug_assert!(
                            branch_v2.key.is_empty(),
                            "Branch at {:?} already has extension key {:?}",
                            last.path,
                            branch_v2.key
                        );
                        branch_v2.key = ext.key;
                        branch_v2.branch_rlp_node = Some(ext.child);
                        last.path = path;
                    }

                    // If we reach here, the extension's child is not a branch in the
                    // result. This happens when the child branch is hashed (not revealed
                    // in the proof). In V2 format, extension nodes are always combined
                    // with their child branch, so we skip extension nodes whose child
                    // isn't revealed.
                }
            }
        }

        result
    }
}

/// Enum representing an MPT trie node.
///
/// This is a V2 representiation, differing from [`TrieNode`] in that branch and extension nodes are
/// compressed into a single node.
#[derive(PartialEq, Eq, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TrieNodeV2 {
    /// Variant representing empty root node.
    EmptyRoot,
    /// Variant representing a [`BranchNodeV2`].
    Branch(BranchNodeV2),
    /// Variant representing a [`LeafNode`].
    Leaf(LeafNode),
    /// Variant representing an [`ExtensionNode`].
    ///
    /// This will only be used for extension nodes for which child is not inlined. This variant
    /// will never be produced by proof workers that will always reveal a full path to a requested
    /// leaf.
    Extension(ExtensionNode),
}

impl Encodable for TrieNodeV2 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Self::EmptyRoot => {
                out.put_u8(EMPTY_STRING_CODE);
            }
            Self::Leaf(leaf) => {
                leaf.as_ref().encode(out);
            }
            Self::Branch(branch) => branch.encode(out),
            Self::Extension(ext) => {
                ext.encode(out);
            }
        }
    }
}

impl Decodable for TrieNodeV2 {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        match TrieNode::decode(buf)? {
            TrieNode::EmptyRoot => Ok(Self::EmptyRoot),
            TrieNode::Leaf(leaf) => Ok(Self::Leaf(leaf)),
            TrieNode::Branch(branch) => Ok(Self::Branch(BranchNodeV2::new(
                Default::default(),
                branch.stack,
                branch.state_mask,
                None,
            ))),
            TrieNode::Extension(ext) => {
                if ext.child.is_hash() {
                    Ok(Self::Extension(ext))
                } else {
                    let Self::Branch(mut branch) = Self::decode(&mut ext.child.as_ref())? else {
                        return Err(alloy_rlp::Error::Custom(
                            "extension node child is not a branch",
                        ));
                    };

                    branch.key = ext.key;

                    Ok(Self::Branch(branch))
                }
            }
        }
    }
}

/// A branch node in an Ethereum Merkle Patricia Trie.
///
/// Branch node is a 17-element array consisting of 16 slots that correspond to each hexadecimal
/// character and an additional slot for a value. We do exclude the node value since all paths have
/// a fixed size.
///
/// This node also encompasses the possible parent extension node of a branch via the `key` field.
#[derive(PartialEq, Eq, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BranchNodeV2 {
    /// The key for the branch's parent extension. if key is empty then the branch does not have a
    /// parent extension.
    pub key: Nibbles,
    /// The collection of RLP encoded children.
    pub stack: Vec<RlpNode>,
    /// The bitmask indicating the presence of children at the respective nibble positions
    pub state_mask: TrieMask,
    /// [`RlpNode`] encoding of the branch node. Always provided when `key` is not empty (i.e this
    /// is an extension node).
    pub branch_rlp_node: Option<RlpNode>,
}

impl fmt::Debug for BranchNodeV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BranchNode")
            .field("key", &self.key)
            .field("stack", &self.stack.iter().map(hex::encode).collect::<Vec<_>>())
            .field("state_mask", &self.state_mask)
            .field("branch_rlp_node", &self.branch_rlp_node)
            .finish()
    }
}

impl BranchNodeV2 {
    /// Creates a new branch node with the given short key, stack, and state mask.
    pub const fn new(
        key: Nibbles,
        stack: Vec<RlpNode>,
        state_mask: TrieMask,
        branch_rlp_node: Option<RlpNode>,
    ) -> Self {
        Self { key, stack, state_mask, branch_rlp_node }
    }
}

impl Encodable for BranchNodeV2 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        if self.key.is_empty() {
            BranchNodeRef::new(&self.stack, self.state_mask).encode(out);
            return;
        }

        let branch_rlp_node = self
            .branch_rlp_node
            .as_ref()
            .expect("branch_rlp_node must always be present for extension nodes");

        ExtensionNodeRef::new(&self.key, branch_rlp_node.as_slice()).encode(out);
    }
}
