use crate::proof_v2::ValueEncoderFut;
use alloy_rlp::Encodable;
use alloy_trie::nodes::{BranchNodeRef, ExtensionNodeRef};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{LeafNodeRef, Nibbles, RlpNode, TrieMask};

/// A trie node which is the child of a branch in the trie.
#[derive(Debug)]
pub(crate) enum ProofTrieBranchChild<RF> {
    /// A leaf node whose value has yet to be calculated and encoded.
    Leaf {
        /// The short key of the leaf.
        short_key: Nibbles,
        /// The [`ValueEncoderFut`] which will encode the leaf's value.
        value: RF,
    },
    /// A child which has already been encoded as an RLP node.
    RlpNode(RlpNode),
}

impl<RF: ValueEncoderFut> ProofTrieBranchChild<RF> {
    /// Converts this child into its RLP node representation.
    ///
    /// For a `Leaf` variant, this will encode the leaf value using the provided buffer
    /// and return the `RlpNode`.
    ///
    /// For an `RlpNode` variant, this returns the RLP node as-is.
    pub(crate) fn into_rlp(self, buf: &mut Vec<u8>) -> Result<RlpNode, StateProofError> {
        match self {
            Self::Leaf { short_key, value } => {
                buf.clear();

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
                Ok(RlpNode::from_rlp(&buf[value_enc_len..]))
            }
            Self::RlpNode(rlp_node) => Ok(rlp_node),
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
    pub(crate) tree_mask: TrieMask,
    /// A subset of `state_mask`. Each bit is set if the hash for the child is cached in the DB.
    pub(crate) hash_mask: TrieMask,
}

impl ProofTrieBranch {
    /// Converts this branch (and its optional extension parent) into its RLP node representation.
    ///
    /// # Arguments
    ///
    /// * `buf` - A mutable buffer for RLP encoding
    /// * `rlp_node_buf` - A reusable buffer for collecting RLP nodes
    /// * `children` - An iterable collection of `ProofTrieBranchChild`s, must have length equal to
    ///   the number of bits set in `state_mask`
    /// * `branch_path` - The full path to this branch
    ///
    /// If `ext_len > 0`, an extension node is constructed with the last `ext_len` nibbles of the
    /// path as its short key, and the branch as its child. The extension's `RlpNode` is
    /// returned. Otherwise, the branch's `RlpNode` is returned directly.
    pub(crate) fn into_rlp<RF: ValueEncoderFut>(
        self,
        buf: &mut Vec<u8>,
        rlp_node_buf: &mut Vec<RlpNode>,
        children: impl IntoIterator<Item = ProofTrieBranchChild<RF>>,
        branch_path: &Nibbles,
    ) -> Result<RlpNode, StateProofError> {
        // Collect children into RlpNode buffer by calling into_rlp on each
        rlp_node_buf.clear();
        for child in children {
            rlp_node_buf.push(child.into_rlp(buf)?);
        }

        debug_assert_eq!(
            rlp_node_buf.len(),
            self.state_mask.count_ones() as usize,
            "children length must match number of bits set in state_mask"
        );

        // Encode the branch node
        buf.clear();
        BranchNodeRef::new(rlp_node_buf, self.state_mask).encode(buf);
        let branch_rlp = RlpNode::from_rlp(buf);

        // If there is no extension parent then return the branch's node directly.
        if self.ext_len == 0 {
            return Ok(branch_rlp)
        }

        // Otherwise compute the extension's short key and return its `RlpNode`
        let start_idx = branch_path.len() - self.ext_len as usize;
        let short_key = branch_path.slice_unchecked(start_idx, branch_path.len());

        buf.clear();
        ExtensionNodeRef::new(&short_key, branch_rlp.as_slice()).encode(buf);
        Ok(RlpNode::from_rlp(buf))
    }
}
