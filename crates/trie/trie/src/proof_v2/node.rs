use crate::proof_v2::DeferredValueEncoder;
use alloy_primitives::Keccak256;
use alloy_rlp::{length_of_length, Encodable, EMPTY_LIST_CODE, EMPTY_STRING_CODE};
use alloy_trie::nodes::{encode_path_leaf, BranchNodeRef, ExtensionNodeRef};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    BranchNodeMasks, BranchNodeV2, LeafNode, LeafNodeRef, Nibbles, ProofTrieNodeV2, RlpNode,
    TrieMask, TrieNodeV2,
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
        node: BranchNodeV2,
        /// Bitmasks carried over from cached `BranchNodeCompact` values, if any.
        masks: Option<BranchNodeMasks>,
    },
    /// A node whose type is not known, as it has already been converted to an [`RlpNode`].
    RlpNode(RlpNode),
}

impl<RF: DeferredValueEncoder> ProofTrieBranchChild<RF> {
    /// Converts this child into its RLP node representation.
    ///
    /// This potentially also returns an `RlpNode` buffer which can be re-used for other
    /// [`ProofTrieBranchChild`]s.
    pub(crate) fn into_rlp(
        self,
        buf: &mut Vec<u8>,
    ) -> Result<(RlpNode, Option<Vec<RlpNode>>), StateProofError> {
        match self {
            Self::Leaf { short_key, value } => {
                // RLP encode the value itself
                value.encode(buf)?;
                let value_enc_len = buf.len();

                if let Some(rlp_node) = hashed_leaf_rlp_node(&short_key, &buf[..value_enc_len]) {
                    return Ok((rlp_node, None))
                }

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
            Self::Branch { node: branch_node, .. } => {
                let rlp_node = branch_v2_rlp_node(&branch_node, buf);
                Ok((rlp_node, Some(branch_node.stack)))
            }
            Self::RlpNode(rlp_node) => Ok((rlp_node, None)),
        }
    }

    /// Converts this child into a [`ProofTrieNodeV2`] having the given path.
    ///
    /// # Panics
    ///
    /// If called on a [`Self::RlpNode`].
    pub(crate) fn into_proof_trie_node(
        self,
        path: Nibbles,
        buf: &mut Vec<u8>,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
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
                (TrieNodeV2::Leaf(LeafNode::new(short_key, rlp_val)), None)
            }
            Self::Branch { node, masks } => (TrieNodeV2::Branch(node), masks),
            Self::RlpNode(_) => panic!("Cannot call `into_proof_trie_node` on RlpNode"),
        };

        Ok(ProofTrieNodeV2 { node, path, masks })
    }

    /// Returns the short key of the child, if it is a leaf or branch, or empty if its a
    /// [`Self::RlpNode`].
    pub(crate) fn short_key(&self) -> &Nibbles {
        match self {
            Self::Leaf { short_key, .. } |
            Self::Branch { node: BranchNodeV2 { key: short_key, .. }, .. } => short_key,
            Self::RlpNode(_) => {
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
            Self::Leaf { short_key, .. } => {
                *short_key = trim_nibbles_prefix(short_key, len);
            }
            Self::Branch { node: BranchNodeV2 { key, branch_rlp_node, .. }, .. } => {
                *key = trim_nibbles_prefix(key, len);
                if key.is_empty() {
                    *branch_rlp_node = None;
                }
            }
            Self::RlpNode(_) => {
                panic!("Cannot call `trim_short_key_prefix` on RlpNode")
            }
        }
    }
}

/// Returns the same [`RlpNode`] as encoding `node` and passing it to [`RlpNode::from_rlp`], but
/// streams large proof-node encodings directly into Keccak instead of first materializing the full
/// node RLP in a growable byte buffer.
pub(crate) fn trie_node_v2_rlp_node(node: &TrieNodeV2, buf: &mut Vec<u8>) -> RlpNode {
    match node {
        TrieNodeV2::EmptyRoot => RlpNode::from_raw(&[EMPTY_STRING_CODE])
            .expect("empty root RLP is one byte and always fits in RlpNode"),
        TrieNodeV2::Leaf(leaf) => leaf_rlp_node(&leaf.key, &leaf.value, buf),
        TrieNodeV2::Branch(branch) => branch_v2_rlp_node(branch, buf),
        TrieNodeV2::Extension(extension) => extension_rlp_node(
            &extension.key,
            extension.child.as_slice(),
            buf,
        ),
    }
}

/// Returns the branch-node child pointer for `stack` and `state_mask`, matching
/// `BranchNodeRef::encode` followed by `RlpNode::from_rlp`.
pub(crate) fn branch_rlp_node(
    stack: &[RlpNode],
    state_mask: TrieMask,
    buf: &mut Vec<u8>,
) -> RlpNode {
    let payload_len = branch_payload_length(stack, state_mask);
    let encoded_len = payload_len + length_of_length(payload_len);

    if encoded_len < 32 {
        buf.clear();
        BranchNodeRef::new(stack, state_mask).encode(buf);
        return RlpNode::from_rlp(buf)
    }

    let mut hasher = Keccak256::new();
    update_rlp_header(&mut hasher, EMPTY_LIST_CODE, payload_len);

    let first_child_idx = first_child_index(stack, state_mask);
    let mut stack_iter = stack[first_child_idx..].iter();
    for nibble in 0u8..16 {
        if state_mask.is_bit_set(nibble) {
            let child = stack_iter.next().expect("branch stack must contain all masked children");
            hasher.update(child.as_slice());
        } else {
            hasher.update([EMPTY_STRING_CODE]);
        }
    }
    hasher.update([EMPTY_STRING_CODE]);

    RlpNode::word_rlp(&hasher.finalize())
}

fn branch_v2_rlp_node(branch: &BranchNodeV2, buf: &mut Vec<u8>) -> RlpNode {
    if branch.key.is_empty() {
        return branch_rlp_node(&branch.stack, branch.state_mask, buf)
    }

    let branch_rlp_node = branch
        .branch_rlp_node
        .as_ref()
        .expect("branch_rlp_node must always be present for extension nodes");
    extension_rlp_node(&branch.key, branch_rlp_node.as_slice(), buf)
}

fn leaf_rlp_node(short_key: &Nibbles, value: &[u8], buf: &mut Vec<u8>) -> RlpNode {
    if let Some(rlp_node) = hashed_leaf_rlp_node(short_key, value) {
        return rlp_node
    }

    buf.clear();
    LeafNodeRef::new(short_key, value).encode(buf);
    RlpNode::from_rlp(buf)
}

fn hashed_leaf_rlp_node(short_key: &Nibbles, value: &[u8]) -> Option<RlpNode> {
    let encoded_path = encode_path_leaf(short_key, true);
    let payload_len = rlp_string_len(&encoded_path) + rlp_string_len(value);
    let encoded_len = payload_len + length_of_length(payload_len);

    if encoded_len < 32 {
        return None
    }

    let mut hasher = Keccak256::new();
    update_rlp_header(&mut hasher, EMPTY_LIST_CODE, payload_len);
    update_rlp_string(&mut hasher, &encoded_path);
    update_rlp_string(&mut hasher, value);
    Some(RlpNode::word_rlp(&hasher.finalize()))
}

fn extension_rlp_node(short_key: &Nibbles, child: &[u8], buf: &mut Vec<u8>) -> RlpNode {
    let encoded_path = encode_path_leaf(short_key, false);
    let payload_len = rlp_string_len(&encoded_path) + child.len();
    let encoded_len = payload_len + length_of_length(payload_len);

    if encoded_len < 32 {
        buf.clear();
        ExtensionNodeRef::new(short_key, child).encode(buf);
        return RlpNode::from_rlp(buf)
    }

    let mut hasher = Keccak256::new();
    update_rlp_header(&mut hasher, EMPTY_LIST_CODE, payload_len);
    update_rlp_string(&mut hasher, &encoded_path);
    hasher.update(child);
    RlpNode::word_rlp(&hasher.finalize())
}

fn branch_payload_length(stack: &[RlpNode], state_mask: TrieMask) -> usize {
    let first_child_idx = first_child_index(stack, state_mask);
    let mut stack_iter = stack[first_child_idx..].iter();
    let mut payload_len = 1;

    for nibble in 0u8..16 {
        if state_mask.is_bit_set(nibble) {
            let child = stack_iter.next().expect("branch stack must contain all masked children");
            payload_len += child.len();
        } else {
            payload_len += 1;
        }
    }

    payload_len
}

fn first_child_index(stack: &[RlpNode], state_mask: TrieMask) -> usize {
    stack.len().checked_sub(state_mask.count_ones() as usize).expect(
        "branch stack length must be at least the number of children in the state mask",
    )
}

fn rlp_string_len(bytes: &[u8]) -> usize {
    if bytes.len() == 1 && bytes[0] < EMPTY_STRING_CODE {
        1
    } else {
        bytes.len() + length_of_length(bytes.len())
    }
}

fn update_rlp_string(hasher: &mut Keccak256, bytes: &[u8]) {
    if bytes.len() == 1 && bytes[0] < EMPTY_STRING_CODE {
        hasher.update(bytes);
    } else {
        update_rlp_header(hasher, EMPTY_STRING_CODE, bytes.len());
        hasher.update(bytes);
    }
}

fn update_rlp_header(hasher: &mut Keccak256, offset: u8, payload_len: usize) {
    if payload_len < 56 {
        hasher.update([offset + payload_len as u8]);
        return
    }

    let len_bytes = payload_len.to_be_bytes();
    let first_non_zero = len_bytes
        .iter()
        .position(|byte| *byte != 0)
        .expect("payload length is at least 56");
    let len_of_len = len_bytes.len() - first_non_zero;

    hasher.update([offset + 55 + len_of_len as u8]);
    hasher.update(&len_bytes[first_non_zero..]);
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
    /// Bitmasks which are subsets of `state_mask`.
    pub(crate) masks: Option<BranchNodeMasks>,
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
    use alloy_primitives::B256;

    fn assert_trie_node_v2_rlp_matches_canonical(node: TrieNodeV2) {
        let mut canonical = Vec::new();
        node.encode(&mut canonical);

        let mut buf = Vec::new();
        assert_eq!(trie_node_v2_rlp_node(&node, &mut buf), RlpNode::from_rlp(&canonical));
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

    #[test]
    fn streamed_rlp_node_matches_canonical_encoding() {
        assert_trie_node_v2_rlp_matches_canonical(TrieNodeV2::EmptyRoot);

        assert_trie_node_v2_rlp_matches_canonical(TrieNodeV2::Leaf(LeafNode::new(
            Nibbles::from_nibbles([1, 2, 3, 4]),
            vec![0xab; 96],
        )));

        let state_mask = TrieMask::new((1 << 0) | (1 << 7) | (1 << 15));
        let stack = vec![
            RlpNode::word_rlp(&B256::repeat_byte(0x11)),
            RlpNode::word_rlp(&B256::repeat_byte(0x77)),
            RlpNode::word_rlp(&B256::repeat_byte(0xff)),
        ];

        assert_trie_node_v2_rlp_matches_canonical(TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::new(),
            stack.clone(),
            state_mask,
            None,
        )));

        let mut branch_rlp = Vec::new();
        BranchNodeRef::new(&stack, state_mask).encode(&mut branch_rlp);
        assert_trie_node_v2_rlp_matches_canonical(TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::from_nibbles([0xa, 0xb, 0xc]),
            stack,
            state_mask,
            Some(RlpNode::from_rlp(&branch_rlp)),
        )));
    }
}
