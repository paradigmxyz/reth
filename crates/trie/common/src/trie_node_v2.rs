//! Version 2 types related to representing nodes in an MPT.

use crate::BranchNodeMasks;
use alloc::vec::Vec;
use alloy_primitives::{hex, B256};
use alloy_rlp::{Decodable, Encodable, EMPTY_STRING_CODE};
use alloy_trie::{
    nodes::{BranchNodeRef, ExtensionNodeRef, LeafNode, RlpNode, TrieNode},
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
}

impl TrieNodeV2 {
    /// Converts this node into its RLP node representation, encoding into the provided buffer.
    ///
    /// Returns the [`RlpNode`] pointing to the encoded data.
    pub fn encode_rlp(&self, buf: &mut Vec<u8>) {
        match self {
            Self::EmptyRoot => {
                buf.push(EMPTY_STRING_CODE);
            }
            Self::Leaf(leaf) => {
                leaf.as_ref().encode(buf);
            }
            Self::Branch(branch) => branch.encode_rlp(buf),
        }
    }

    /// Attempts to decode a trie node from the given buffer.
    ///
    /// This returns `None` in cases when provided buffer contains a valid RLP node that can't be
    /// represented as [`TrieNodeV2`]. Specifically, this returns `None` for extension nodes which
    /// attach to branches that are stored as hashes.
    pub fn try_decode(buf: &mut &[u8]) -> Result<Option<Self>, alloy_rlp::Error> {
        match TrieNode::decode(buf)? {
            TrieNode::EmptyRoot => Ok(Some(Self::EmptyRoot)),
            TrieNode::Leaf(leaf) => Ok(Some(Self::Leaf(leaf))),
            TrieNode::Branch(branch) => Ok(Some(Self::Branch(BranchNodeV2::new(
                Default::default(),
                branch.stack,
                branch.state_mask,
            )))),
            TrieNode::Extension(ext) => {
                if ext.child.as_ref().len() == B256::len_bytes() + 1 {
                    // Extension node with a hash child.
                    Ok(None)
                } else {
                    let Self::Branch(mut branch) = Self::try_decode(&mut ext.child.as_ref())?
                        .ok_or(alloy_rlp::Error::Custom("extension node child is not a branch"))?
                    else {
                        return Err(alloy_rlp::Error::Custom(
                            "extension node child is not a branch",
                        ));
                    };

                    branch.key = ext.key;

                    Ok(Some(Self::Branch(branch)))
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
}

impl fmt::Debug for BranchNodeV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BranchNode")
            .field("key", &self.key)
            .field("stack", &self.stack.iter().map(hex::encode).collect::<Vec<_>>())
            .field("state_mask", &self.state_mask)
            .finish()
    }
}

impl BranchNodeV2 {
    /// Creates a new branch node with the given short key, stack, and state mask.
    pub const fn new(key: Nibbles, stack: Vec<RlpNode>, state_mask: TrieMask) -> Self {
        Self { key, stack, state_mask }
    }

    /// Converts this node into its RLP node representation, encoding into the provided buffer.
    ///
    /// Returns the [`RlpNode`] pointing to the encoded data.
    pub fn encode_rlp(&self, buf: &mut Vec<u8>) {
        // We always start by encoding the branch
        let branch_ref = BranchNodeRef::new(&self.stack, self.state_mask);
        branch_ref.encode(buf);

        // If key is empty then there is no parent extension, the encoded branch is the result
        if self.key.is_empty() {
            return
        }

        // Convert branch to `RlpNode`. This will hash it if it's >32 bytes encoded.
        let branch_rlp_node = RlpNode::from_rlp(buf);

        // Clear the buffer and encode the extension into it.
        buf.clear();
        ExtensionNodeRef::new(&self.key, branch_rlp_node.as_slice()).encode(buf);
    }
}
