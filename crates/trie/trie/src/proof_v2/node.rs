use super::DeferredValueEncoder;
use alloy_primitives::B256;
use alloy_rlp::Encodable;
use alloy_trie::TrieMask;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    BranchNodeMasks, BranchNodeRef, BranchNodeV2, ExtensionNodeRef, LeafNode, LeafNodeRef, Nibbles,
    ProofTrieNodeV2, RlpNode, TrieNodeV2,
};

/// A complete subtree whose absolute path remains available until its parent is known.
#[derive(Debug)]
pub(super) struct ProofNode<D> {
    pub(super) path: Nibbles,
    /// Deepest target prefix shared by any input in this subtree.
    pub(super) retain_depth: usize,
    pub(super) kind: ProofNodeKind<D>,
}

impl<D: DeferredValueEncoder> ProofNode<D> {
    /// Encodes this node at its parent's child edge. Branch buffers can be reused after encoding.
    pub(super) fn into_rlp(
        self,
        parent_len: usize,
        buf: &mut Vec<u8>,
    ) -> Result<(RlpNode, Option<Vec<RlpNode>>), StateProofError> {
        let short_key = self.path.slice_unchecked(parent_len, self.path.len());
        let (node, freed) = match self.kind {
            ProofNodeKind::Leaf(value) => {
                value.encode(buf)?;
                let value_len = buf.len();
                let leaf_len = LeafNodeRef::new(&short_key, buf).length();
                buf.resize(value_len + leaf_len, 0);
                // The value stays in the first part while the leaf is encoded into the second.
                let (value_buf, mut leaf_buf) = buf.split_at_mut(value_len);
                LeafNodeRef::new(&short_key, value_buf).encode(&mut leaf_buf);
                return Ok((RlpNode::from_rlp(&buf[value_len..]), None))
            }
            ProofNodeKind::Branch { children, state_mask, .. } => {
                BranchNodeRef::new(&children, state_mask).encode(buf);
                (RlpNode::from_rlp(buf), Some(children))
            }
            ProofNodeKind::Hash { hash, .. } => (RlpNode::word_rlp(&hash), None),
        };
        if short_key.is_empty() {
            return Ok((node, freed))
        }
        buf.clear();
        ExtensionNodeRef::new(&short_key, node.as_slice()).encode(buf);
        Ok((RlpNode::from_rlp(buf), freed))
    }

    pub(super) fn into_proof(
        self,
        path: Nibbles,
        buf: &mut Vec<u8>,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
        let short_key = self.path.slice_unchecked(path.len(), self.path.len());
        let (node, masks) = match self.kind {
            ProofNodeKind::Leaf(value) => {
                value.encode(buf)?;
                // A copy keeps the scratch buffer's capacity for later leaves.
                (TrieNodeV2::Leaf(LeafNode::new(short_key, buf.clone())), None)
            }
            ProofNodeKind::Branch { children, state_mask, masks } => {
                let branch_rlp = if short_key.is_empty() {
                    None
                } else {
                    BranchNodeRef::new(&children, state_mask).encode(buf);
                    Some(RlpNode::from_rlp(buf))
                };
                (
                    TrieNodeV2::Branch(BranchNodeV2::new(
                        short_key, children, state_mask, branch_rlp,
                    )),
                    (!masks.is_empty()).then_some(masks),
                )
            }
            ProofNodeKind::Hash { .. } => {
                return Err(StateProofError::TrieInconsistency(
                    "cannot convert a cached hash to a proof node".to_string(),
                ))
            }
        };
        Ok(ProofTrieNodeV2 { path, node, masks })
    }
}

#[derive(Debug)]
pub(super) enum ProofNodeKind<D> {
    Leaf(D),
    Branch { children: Vec<RlpNode>, state_mask: TrieMask, masks: BranchNodeMasks },
    Hash { hash: B256, stored: bool },
}
