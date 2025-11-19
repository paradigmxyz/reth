use crate::proof_v2::DeferredValueEncoder;
use alloy_rlp::Encodable;
use alloy_trie::nodes::ExtensionNodeRef;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    BranchNode, ExtensionNode, LeafNode, LeafNodeRef, Nibbles, RlpNode, TrieMask, TrieNode,
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
    /// An extension node whose child branch has not yet been converted to an [`RlpNode`]
    Extension {
        /// The short key of the leaf.
        short_key: Nibbles,
        /// The node of the child branch.
        child: BranchNode,
    },
    /// A branch node whose children have already been flattened into [`RlpNode`]s.
    Branch(BranchNode),
}

impl<RF: DeferredValueEncoder> ProofTrieBranchChild<RF> {
    /// Converts this child into its RLP node representation. This potentially also returns an
    /// `RlpNode` buffer which can be re-used for other `ProofTrieBranchChild`s.
    pub(crate) fn into_rlp(
        self,
        buf: &mut Vec<u8>,
    ) -> Result<(RlpNode, Option<Vec<RlpNode>>), StateProofError> {
        buf.clear();
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
                let (branch_rlp, rlp_buf) = Self::Branch(child).into_rlp(buf)?;
                buf.clear();

                ExtensionNodeRef::new(&short_key, branch_rlp.as_slice()).encode(buf);
                Ok((RlpNode::from_rlp(buf), rlp_buf))
            }
            Self::Branch(branch_node) => {
                branch_node.encode(buf);
                Ok((RlpNode::from_rlp(buf), Some(branch_node.stack)))
            }
        }
    }

    /// Converts this child into a [`TrieNode`].
    pub(crate) fn into_trie_node(self, buf: &mut Vec<u8>) -> Result<TrieNode, StateProofError> {
        buf.clear();
        match self {
            Self::Leaf { short_key, value } => {
                value.encode(buf)?;
                Ok(TrieNode::Leaf(LeafNode::new(short_key, core::mem::take(buf))))
            }
            Self::Extension { short_key, child } => {
                child.encode(buf);
                let child_rlp_node = RlpNode::from_rlp(buf);
                Ok(TrieNode::Extension(ExtensionNode { key: short_key, child: child_rlp_node }))
            }
            Self::Branch(branch_node) => Ok(TrieNode::Branch(branch_node)),
        }
    }

    /// Returns the short key of the child, if it is a leaf or extension, or empty if its a
    /// [`Self::Branch`].
    pub(crate) fn short_key(&self) -> &Nibbles {
        match self {
            Self::Leaf { short_key, .. } | Self::Extension { short_key, .. } => short_key,
            Self::Branch(_) => {
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
    /// - If the node is a [`Self::Branch`]
    pub(crate) fn trim_short_key_prefix(&mut self, len: usize) {
        match self {
            Self::Extension { short_key, child } if short_key.len() == len => {
                *self = Self::Branch(core::mem::take(child));
            }
            Self::Leaf { short_key, .. } | Self::Extension { short_key, .. } => {
                *short_key = trim_nibbles_prefix(short_key, len);
            }
            Self::Branch(_) => {
                panic!("Cannot call `trim_short_key_prefix` on Branch")
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
