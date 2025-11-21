use crate::proof_v2::DeferredValueEncoder;
use alloy_rlp::Encodable;
use alloy_trie::nodes::ExtensionNodeRef;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    BranchNode, ExtensionNode, LeafNode, LeafNodeRef, Nibbles, ProofTrieNode, RlpNode, TrieMask,
    TrieMasks, TrieNode,
};

/// A trie node which is the child of a branch in the trie.
#[derive(Debug)]
pub(crate) enum ProofTrieBranchChild<RF> {
    /// A leaf node whose value has yet to be calculated and encoded.
    Leaf {
        /// The short key of the leaf.
        short_key: Nibbles,
        /// The [`DeferredValueEncoder`] which will encode the leaf's value.
        value: RF,
    },
    /// An extension node whose child branch has been converted to an [`RlpNode`]
    Extension {
        /// The short key of the leaf.
        short_key: Nibbles,
        /// The [`RlpNode`] of the child branch.
        child: RlpNode,
    },
    /// A branch node whose children have already been flattened into [`RlpNode`]s.
    Branch(BranchNode),
    // A node whose type is not known, as it has already been converted to an [`RlpNode`].
    RlpNode(RlpNode),
}

impl<RF: DeferredValueEncoder> ProofTrieBranchChild<RF> {
    /// Converts this child into its RLP node representation. This potentially also returns an
    /// `RlpNode` buffer which can be re-used for other [`ProofTrieBranchChild`]s.
    pub(crate) fn into_rlp(
        self,
        buf: &mut Vec<u8>,
    ) -> Result<(RlpNode, Option<Vec<RlpNode>>), StateProofError> {
        match self {
            Self::Leaf { short_key, value } => {
                // RLP encode the value itself
                value.encode(buf)?;
                let value_enc_len = buf.len();

                // Determine the required buffer size for the encoded leaf
                let leaf_enc_len = LeafNodeRef::new(&short_key, buf).length();

                // We want to re-use buf for the encoding of the leaf node as well. To do this we
                // will keep appending to it, leaving the already encoded value in-place. First we
                // must ensure the buffer is big enough, then we'll split.
                buf.resize(value_enc_len + leaf_enc_len, 0);

                // SAFETY we have just resized the above to be greater than `value_enc_len`, so it
                // must be in-bounds.
                let (value_buf, mut leaf_buf) =
                    unsafe { buf.split_at_mut_unchecked(value_enc_len) };

                // Encode the leaf into the right side of the split buffer, and return the RlpNode.
                LeafNodeRef::new(&short_key, value_buf).encode(&mut leaf_buf);
                Ok((RlpNode::from_rlp(&buf[value_enc_len..]), None))
            }
            Self::Extension { short_key, child } => {
                ExtensionNodeRef::new(&short_key, child.as_slice()).encode(buf);
                Ok((RlpNode::from_rlp(buf), None))
            }
            Self::Branch(branch_node) => {
                branch_node.encode(buf);
                Ok((RlpNode::from_rlp(buf), Some(branch_node.stack)))
            }
            Self::RlpNode(rlp_node) => Ok((rlp_node, None)),
        }
    }

    /// Converts this child into a [`ProofTrieNode`] having the given path.
    ///
    /// # Panics
    ///
    /// If called on a [`Self::RlpNode`].
    pub(crate) fn into_proof_trie_node(
        self,
        path: Nibbles,
        buf: &mut Vec<u8>,
    ) -> Result<ProofTrieNode, StateProofError> {
        let (node, masks) = match self {
            Self::Leaf { short_key, value } => {
                value.encode(buf)?;
                (TrieNode::Leaf(LeafNode::new(short_key, core::mem::take(buf))), TrieMasks::none())
            }
            Self::Extension { short_key, child } => {
                (TrieNode::Extension(ExtensionNode { key: short_key, child }), TrieMasks::none())
            }
            // TODO store trie masks on branch
            Self::Branch(branch_node) => (TrieNode::Branch(branch_node), TrieMasks::none()),
            Self::RlpNode(_) => panic!("Cannot call `into_proof_trie_node` on RlpNode"),
        };

        // Encode the `TrieNode` to the buffer, so we can return the `RlpNode` for it at the end.
        buf.clear();
        node.encode(buf);

        Ok(ProofTrieNode { node, path, masks })
    }

    /// Returns the short key of the child, if it is a leaf or extension, or empty if its a
    /// [`Self::Branch`] or [`Self::RlpNode`].
    pub(crate) fn short_key(&self) -> &Nibbles {
        match self {
            Self::Leaf { short_key, .. } | Self::Extension { short_key, .. } => short_key,
            Self::Branch(_) | Self::RlpNode(_) => {
                static EMPTY_NIBBLES: Nibbles = Nibbles::new();
                &EMPTY_NIBBLES
            }
        }
    }

    /// Trims the given number of nibbles off the head of the short key.
    ///
    /// If the node is an extension and the given length is the same as its short key length, then
    /// the node is replaced with its child.
    ///
    /// # Panics
    ///
    /// - If the given len is longer than the short key
    /// - If the given len is the same as the length of a leaf's short key
    /// - If the node is a [`Self::Branch`] or [`Self::RlpNode`]
    pub(crate) fn trim_short_key_prefix(&mut self, len: usize) {
        match self {
            Self::Extension { short_key, child } if short_key.len() == len => {
                *self = Self::RlpNode(core::mem::take(child));
            }
            Self::Leaf { short_key, .. } | Self::Extension { short_key, .. } => {
                *short_key = trim_nibbles_prefix(short_key, len);
            }
            Self::Branch(_) | Self::RlpNode(_) => {
                panic!("Cannot call `trim_short_key_prefix` on Branch or RlpNode")
            }
        }
    }
}

/// A single branch in the trie which is under construction. The actual child nodes of the branch
/// will be tracked as [`ProofTrieBranchChild`]s on a stack.
#[derive(Debug)]
pub(crate) struct ProofTrieBranch {
    /// The length of the parent extension node's short key. If zero then the branch's parent is
    /// not an extension but instead another branch.
    pub(crate) ext_len: u8,
    /// A mask tracking which child nibbles are set on the branch so far. There will be a single
    /// child on the stack for each set bit.
    pub(crate) state_mask: TrieMask,
    /// A subset of `state_mask`. Each bit is set if the `state_mask` bit is set and:
    /// - The child is a branch which is stored in the DB.
    /// - The child is an extension whose child branch is stored in the DB.
    #[expect(unused)]
    pub(crate) tree_mask: TrieMask,
    /// A subset of `state_mask`. Each bit is set if the hash for the child is cached in the DB.
    #[expect(unused)]
    pub(crate) hash_mask: TrieMask,
}

/// Trims the first `len` nibbles from the head of the given `Nibbles`.
///
/// # Panics
///
/// Panics if the given `len` is greater than the length of the `Nibbles`.
pub(crate) fn trim_nibbles_prefix(n: &Nibbles, len: usize) -> Nibbles {
    debug_assert!(n.len() >= len);
    n.slice_unchecked(len, n.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_nibbles_prefix_basic() {
        // Create nibbles [1, 2, 3, 4, 5, 6]
        let nibbles = Nibbles::from_nibbles([1, 2, 3, 4, 5, 6]);

        // Trim first 2 nibbles
        let trimmed = trim_nibbles_prefix(&nibbles, 2);
        assert_eq!(trimmed.len(), 4);

        // Verify the remaining nibbles are [3, 4, 5, 6]
        assert_eq!(trimmed.get(0), Some(3));
        assert_eq!(trimmed.get(1), Some(4));
        assert_eq!(trimmed.get(2), Some(5));
        assert_eq!(trimmed.get(3), Some(6));
    }

    #[test]
    fn test_trim_nibbles_prefix_zero() {
        // Create nibbles [10, 11, 12, 13]
        let nibbles = Nibbles::from_nibbles([10, 11, 12, 13]);

        // Trim zero nibbles - should return identical nibbles
        let trimmed = trim_nibbles_prefix(&nibbles, 0);
        assert_eq!(trimmed, nibbles);
    }

    #[test]
    fn test_trim_nibbles_prefix_all() {
        // Create nibbles [1, 2, 3, 4]
        let nibbles = Nibbles::from_nibbles([1, 2, 3, 4]);

        // Trim all nibbles - should return empty
        let trimmed = trim_nibbles_prefix(&nibbles, 4);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn test_trim_nibbles_prefix_empty() {
        // Create empty nibbles
        let nibbles = Nibbles::new();

        // Trim zero from empty - should return empty
        let trimmed = trim_nibbles_prefix(&nibbles, 0);
        assert!(trimmed.is_empty());
    }
}
