use crate::proof_v2::DeferredValueEncoder;
use alloy_rlp::{bytes::buf::UninitSlice, BufMut, Encodable};
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

                // Determine the required buffer size for the encoded leaf
                let leaf_enc_len = LeafNodeRef::new(&short_key, buf).length();

                // Encode the leaf directly into the spare capacity after the already encoded
                // value. The RLP encoder initializes the bytes before advancing the writer.
                let mut leaf_buf = VecSpareBuf::new(buf, leaf_enc_len);
                let value_buf = &buf[..value_enc_len];
                LeafNodeRef::new(&short_key, value_buf).encode(&mut leaf_buf);
                let written = leaf_buf.written();
                debug_assert_eq!(written, leaf_enc_len);

                // SAFETY: `VecSpareBuf` reserved `leaf_enc_len` bytes and its `BufMut`
                // implementation only advances after those bytes have been initialized.
                unsafe { buf.set_len(value_enc_len + written) };
                Ok((RlpNode::from_rlp(&buf[value_enc_len..]), None))
            }
            Self::Branch { node: branch_node, .. } => {
                branch_node.encode(buf);
                Ok((RlpNode::from_rlp(buf), Some(branch_node.stack)))
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

/// A `BufMut` writer over the spare capacity at the end of a `Vec<u8>`.
struct VecSpareBuf {
    ptr: *mut u8,
    written: usize,
    cap: usize,
}

impl VecSpareBuf {
    fn new(buf: &mut Vec<u8>, cap: usize) -> Self {
        buf.reserve(cap);
        Self { ptr: unsafe { buf.as_mut_ptr().add(buf.len()) }, written: 0, cap }
    }

    const fn written(&self) -> usize {
        self.written
    }
}

// SAFETY: `chunk_mut` returns the unwritten tail of the reserved spare capacity and
// `advance_mut` never advances beyond it. Callers that use `BufMut` are responsible for
// initializing bytes before advancing, which is the contract relied on by `Vec<u8>`'s own impl.
unsafe impl BufMut for VecSpareBuf {
    #[inline]
    fn remaining_mut(&self) -> usize {
        self.cap - self.written
    }

    #[inline]
    fn chunk_mut(&mut self) -> &mut UninitSlice {
        unsafe {
            UninitSlice::from_raw_parts_mut(
                self.ptr.add(self.written),
                self.cap - self.written,
            )
        }
    }

    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining_mut());
        self.written += cnt;
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

    #[derive(Clone)]
    struct RawValue(Vec<u8>);

    impl DeferredValueEncoder for RawValue {
        fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
            buf.extend_from_slice(&self.0);
            Ok(())
        }
    }

    #[test]
    fn leaf_into_rlp_matches_initialized_split_encoding() {
        let short_key = Nibbles::from_nibbles([1, 2, 3, 4, 5]);
        let value = vec![0xc8, 0x83, b'f', b'o', b'o', 0x83, b'b', b'a', b'r'];

        let mut expected_buf = value.clone();
        let value_enc_len = expected_buf.len();
        let leaf_enc_len = LeafNodeRef::new(&short_key, &expected_buf).length();
        expected_buf.resize(value_enc_len + leaf_enc_len, 0);
        let (value_buf, mut leaf_buf) =
            unsafe { expected_buf.split_at_mut_unchecked(value_enc_len) };
        LeafNodeRef::new(&short_key, value_buf).encode(&mut leaf_buf);
        let expected = RlpNode::from_rlp(&expected_buf[value_enc_len..]);

        let mut buf = Vec::with_capacity(value.len() + leaf_enc_len);
        let (actual, freed) =
            ProofTrieBranchChild::Leaf { short_key, value: RawValue(value) }
                .into_rlp(&mut buf)
                .unwrap();

        assert_eq!(actual, expected);
        assert!(freed.is_none());
        assert_eq!(buf.len(), value_enc_len + leaf_enc_len);
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
