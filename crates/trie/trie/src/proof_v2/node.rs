use crate::proof_v2::DeferredValueEncoder;
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    BranchNode, BranchNodeMasks, ExtensionNode, ExtensionNodeRef, LeafNode, LeafNodeRef, Nibbles,
    ProofTrieNode, RlpNode, TrieMask, TrieNode,
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
    /// A branch node whose children have already been flattened into [`RlpNode`]s.
    Branch {
        /// The node itself, for use during RLP encoding.
        node: BranchNode,
        /// Short key of the separate parent extension, if any.
        short_key: Nibbles,
        /// Bitmasks carried over from cached `BranchNodeCompact` values, if any.
        masks: Option<BranchNodeMasks>,
    },
    /// A node whose type is not known, as it has already been converted to an [`RlpNode`].
    RlpNode {
        /// The RLP-encoded node.
        node: RlpNode,
        /// The path from the parent branch's child nibble to the encoded node. This field can only
        /// be set if `node` was sourced from the hashes of a cached branch, and therefore we know
        /// that it is a blinded branch.
        short_key: Nibbles,
        /// Whether this node contributes to its parent's hash mask when it is a direct child.
        hash_mask_bit: bool,
        /// Whether this node contributes to its parent's tree mask.
        tree_mask_bit: bool,
    },
}

/// Result of converting a branch child into its RLP node, mask bits, and reusable child buffer.
pub(crate) type ChildIntoRlpResult =
    Result<(RlpNode, bool, bool, Option<Vec<RlpNode>>), StateProofError>;

impl<RF: DeferredValueEncoder> ProofTrieBranchChild<RF> {
    /// Converts this child into its RLP node representation.
    ///
    /// This potentially also returns an `RlpNode` buffer which can be re-used for other
    /// [`ProofTrieBranchChild`]s.
    /// Converts this child into its RLP node representation, also returning its
    /// `(hash_mask_bit, tree_mask_bit)` contribution and any reusable `RlpNode` buffer.
    #[inline]
    pub(crate) fn into_rlp(
        self,
        buf: &mut Vec<u8>,
        leaf_value_buf: &mut Vec<u8>,
    ) -> ChildIntoRlpResult {
        match self {
            Self::Leaf { short_key, value } => {
                leaf_value_buf.clear();
                value.encode(leaf_value_buf)?;
                LeafNodeRef::new(&short_key, leaf_value_buf).encode(buf);
                Ok((RlpNode::from_rlp(buf), false, false, None))
            }
            Self::Branch { node, short_key, masks } => {
                node.encode(buf);
                let tree_mask_bit = masks.is_some_and(|m| !m.is_empty());
                let is_hash = buf.len() >= 32;
                let branch_rlp = RlpNode::from_rlp(buf);
                if short_key.is_empty() {
                    return Ok((branch_rlp, is_hash, tree_mask_bit, Some(node.stack)));
                }
                buf.clear();
                ExtensionNodeRef::new(&short_key, branch_rlp.as_slice()).encode(buf);
                Ok((RlpNode::from_rlp(buf), false, tree_mask_bit, Some(node.stack)))
            }
            Self::RlpNode { node, short_key, hash_mask_bit, tree_mask_bit } => {
                if short_key.is_empty() {
                    return Ok((node, hash_mask_bit, tree_mask_bit, None))
                }

                // Only branch hashes sourced from a cached hash mask can have an external short
                // key. Other committed nodes already encode their key internally.
                debug_assert!(hash_mask_bit);
                ExtensionNodeRef::new(&short_key, node.as_slice()).encode(buf);
                Ok((RlpNode::from_rlp(buf), false, tree_mask_bit, None))
            }
        }
    }

    /// Converts this child into a proof node and its separate child branch, if any.
    ///
    /// The optional branch precedes the returned extension in depth-first order.
    ///
    /// # Errors
    ///
    /// Returns [`StateProofError::TrieInconsistency`] if called on a [`Self::RlpNode`].
    pub(crate) fn into_proof_trie_node(
        self,
        path: Nibbles,
        buf: &mut Vec<u8>,
    ) -> Result<(ProofTrieNode, Option<ProofTrieNode>), StateProofError> {
        let (node, masks) = match self {
            Self::Leaf { short_key, value } => {
                value.encode(buf)?;
                // Counter-intuitively a clone is better here than a `core::mem::take`. If we take
                // the buffer then future RLP-encodes will need to re-allocate a new one, and
                // RLP-encodes after those may need a bigger buffer and therefore re-alloc again.
                //
                // By cloning here we do a single allocation of exactly the size we need to take
                // this value, and the passed in buffer can remain with whatever large capacity it
                // already has.
                let rlp_val = buf.clone();
                (TrieNode::Leaf(LeafNode::new(short_key, rlp_val)), None)
            }
            Self::Branch { node, short_key, masks } => {
                if short_key.is_empty() {
                    (TrieNode::Branch(node), masks)
                } else {
                    node.encode(buf);
                    let child = RlpNode::from_rlp(buf);
                    let branch = ProofTrieNode {
                        path: path.join(&short_key),
                        node: TrieNode::Branch(node),
                        masks,
                    };
                    let extension = ProofTrieNode {
                        path,
                        node: TrieNode::Extension(ExtensionNode::new(short_key, child)),
                        masks: None,
                    };
                    return Ok((extension, Some(branch)));
                }
            }
            // Cached hashes cannot be retained as proof nodes: targeted children are recalculated,
            // while untargeted children are either combined into a branch or discarded. Reaching
            // this arm means inconsistent cached trie data left a blinded node as the local root.
            Self::RlpNode { .. } => {
                return Err(StateProofError::TrieInconsistency(
                    "cannot convert RLP node to proof node".to_string(),
                ))
            }
        };

        Ok((ProofTrieNode { node, path, masks }, None))
    }

    /// Returns the child's short key.
    #[inline]
    pub(crate) const fn short_key(&self) -> &Nibbles {
        match self {
            Self::Leaf { short_key, .. } |
            Self::Branch { short_key, .. } |
            Self::RlpNode { short_key, .. } => short_key,
        }
    }

    /// Returns this child's hash and tree mask contributions.
    #[inline]
    pub(crate) fn mask_bits(&self) -> (bool, bool) {
        match self {
            Self::Leaf { .. } => (false, false),
            Self::Branch { node, short_key, masks } => (
                short_key.is_empty() && node.length() >= 32,
                masks.is_some_and(|masks| !masks.is_empty()),
            ),
            Self::RlpNode { short_key, hash_mask_bit, tree_mask_bit, .. } => {
                (*hash_mask_bit && short_key.is_empty(), *tree_mask_bit)
            }
        }
    }

    /// Trims the given number of nibbles off the head of the short key.
    ///
    /// # Panics
    ///
    /// - If the given len is longer than the short key
    #[inline]
    pub(crate) fn trim_short_key_prefix(&mut self, len: usize) {
        match self {
            Self::Leaf { short_key, .. } |
            Self::Branch { short_key, .. } |
            Self::RlpNode { short_key, .. } => {
                *short_key = trim_nibbles_prefix(short_key, len);
            }
        }
    }

    #[inline]
    pub(crate) const fn set_short_key(&mut self, new_short_key: Nibbles) {
        match self {
            Self::Leaf { short_key, .. } |
            Self::Branch { short_key, .. } |
            Self::RlpNode { short_key, .. } => {
                *short_key = new_short_key;
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
}

/// Trims the first `len` nibbles from the head of the given `Nibbles`.
///
/// # Panics
///
/// Panics if the given `len` is greater than the length of the `Nibbles`.
#[inline]
pub(crate) fn trim_nibbles_prefix(n: &Nibbles, len: usize) -> Nibbles {
    debug_assert!(n.len() >= len);
    n.slice_unchecked(len, n.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof_v2::value::StorageDeferredValueEncoder;
    use alloy_primitives::B256;

    #[test]
    fn branch_proofs_keep_extensions_separate() {
        for stack in [
            vec![RlpNode::from_rlp(&[0xc2, 0x20, 0x01]); 2],
            vec![RlpNode::word_rlp(&B256::repeat_byte(0x11)); 2],
        ] {
            let node = BranchNode::new(stack, TrieMask::new(0b11));
            let encoded_branch = alloy_rlp::encode(&node);
            let branch_rlp = RlpNode::from_rlp(&encoded_branch);
            let key = Nibbles::from_nibbles([2, 3]);
            let extension = ExtensionNode::new(key, branch_rlp);
            let masks =
                Some(BranchNodeMasks { hash_mask: TrieMask::new(1), tree_mask: TrieMask::new(2) });
            let child = ProofTrieBranchChild::<StorageDeferredValueEncoder>::Branch {
                node: node.clone(),
                short_key: key,
                masks,
            };
            let path = Nibbles::from_nibbles([1]);
            let (proof, branch) = child.into_proof_trie_node(path, &mut Vec::new()).unwrap();

            assert_eq!(
                proof,
                ProofTrieNode { path, node: TrieNode::Extension(extension.clone()), masks: None }
            );
            assert_eq!(
                branch,
                Some(ProofTrieNode {
                    path: path.join(&key),
                    node: TrieNode::Branch(node.clone()),
                    masks,
                })
            );
            let child = ProofTrieBranchChild::<StorageDeferredValueEncoder>::Branch {
                node,
                short_key: key,
                masks,
            };
            let (rlp, _, _, freed) = child.into_rlp(&mut Vec::new(), &mut Vec::new()).unwrap();
            assert_eq!(rlp, RlpNode::from_rlp(&alloy_rlp::encode(extension)));
            assert_eq!(freed.unwrap().len(), 2);
        }
    }

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
