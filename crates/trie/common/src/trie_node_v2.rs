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
                        node: TrieNodeV2::Branch(BranchNodeV2::new(
                            Nibbles::new(),
                            branch.stack,
                            branch.state_mask,
                        )),
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
                        *branch_v2 = BranchNodeV2::new(
                            ext.key,
                            core::mem::take(&mut branch_v2.stack),
                            branch_v2.state_mask,
                        );
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
    fn length(&self) -> usize {
        match self {
            Self::EmptyRoot => 1,
            Self::Leaf(leaf) => leaf.as_ref().length(),
            Self::Branch(branch) => branch.length(),
            Self::Extension(ext) => ext.length(),
        }
    }

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
            ))),
            TrieNode::Extension(ext) => {
                if ext.child.is_hash() {
                    Ok(Self::Extension(ext))
                } else {
                    let TrieNode::Branch(branch) = TrieNode::decode(&mut ext.child.as_ref())?
                    else {
                        return Err(alloy_rlp::Error::Custom(
                            "extension node child is not a branch",
                        ));
                    };

                    Ok(Self::Branch(BranchNodeV2::new(ext.key, branch.stack, branch.state_mask)))
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
#[cfg_attr(feature = "serde", serde(try_from = "BranchNodeFields"))]
pub struct BranchNodeV2 {
    /// The key for the branch's parent extension. if key is empty then the branch does not have a
    /// parent extension.
    key: Nibbles,
    /// The collection of RLP encoded children.
    stack: Vec<RlpNode>,
    /// The bitmask indicating the presence of children at the respective nibble positions.
    state_mask: TrieMask,
    /// [`RlpNode`] encoding of the branch node. Always provided when `key` is not empty (i.e this
    /// is an extension node).
    branch_rlp_node: Option<RlpNode>,
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
    /// Creates a branch, computing its reference when it has a parent extension.
    ///
    /// # Panics
    ///
    /// Panics if the stack length differs from the number of set bits in `state_mask`.
    pub fn new(key: Nibbles, stack: Vec<RlpNode>, state_mask: TrieMask) -> Self {
        assert_eq!(
            stack.len(),
            state_mask.count_bits() as usize,
            "branch stack must match state mask"
        );
        let branch_rlp_node = (!key.is_empty()).then(|| {
            // At most 16 33-byte child references, an empty value, and a 3-byte list header.
            let mut buffer = [0; 532];
            let mut out = buffer.as_mut_slice();
            BranchNodeRef::new(&stack, state_mask).encode(&mut out);
            let len = 532 - out.len();
            RlpNode::from_rlp(&buffer[..len])
        });
        Self { key, stack, state_mask, branch_rlp_node }
    }

    /// Returns the parent extension key, or an empty key for a bare branch.
    pub const fn key(&self) -> &Nibbles {
        &self.key
    }

    /// Returns the children in nibble order.
    pub fn stack(&self) -> &[RlpNode] {
        &self.stack
    }

    /// Consumes the node and returns its children buffer for reuse.
    pub fn into_stack(self) -> Vec<RlpNode> {
        self.stack
    }

    /// Returns the mask of present children.
    pub const fn state_mask(&self) -> TrieMask {
        self.state_mask
    }

    /// Returns the branch reference used by the parent extension, if present.
    pub const fn branch_rlp_node(&self) -> Option<&RlpNode> {
        self.branch_rlp_node.as_ref()
    }

    /// Removes a prefix from the parent extension key.
    ///
    /// # Panics
    ///
    /// Panics if `len` exceeds the key length.
    pub fn trim_key_prefix(&mut self, len: usize) {
        self.key = self.key.slice(len..);
        if self.key.is_empty() {
            self.branch_rlp_node = None;
        }
    }
}

#[cfg(feature = "serde")]
#[derive(serde::Deserialize)]
struct BranchNodeFields {
    key: Nibbles,
    stack: Vec<RlpNode>,
    state_mask: TrieMask,
    branch_rlp_node: Option<RlpNode>,
}

#[cfg(feature = "serde")]
impl TryFrom<BranchNodeFields> for BranchNodeV2 {
    type Error = &'static str;

    fn try_from(fields: BranchNodeFields) -> Result<Self, Self::Error> {
        if fields.stack.len() != fields.state_mask.count_bits() as usize {
            return Err("branch stack must match state mask")
        }
        let node = Self::new(fields.key, fields.stack, fields.state_mask);
        if node.branch_rlp_node != fields.branch_rlp_node {
            return Err("branch reference must match its children and extension key")
        }
        Ok(node)
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

    fn length(&self) -> usize {
        if self.key.is_empty() {
            return BranchNodeRef::new(&self.stack, self.state_mask).length()
        }

        let branch_rlp_node = self
            .branch_rlp_node
            .as_ref()
            .expect("branch_rlp_node must always be present for extension nodes");

        ExtensionNodeRef::new(&self.key, branch_rlp_node.as_slice()).length()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use proptest::prelude::*;

    fn assert_roundtrip_and_length(node: TrieNodeV2) {
        let encoded = alloy_rlp::encode(&node);
        assert_eq!(node.length(), encoded.len());
        let mut buf = encoded.as_slice();
        assert_eq!(TrieNodeV2::decode(&mut buf).unwrap(), node);
        assert!(buf.is_empty());
    }

    proptest! {
        #[test]
        fn inline_extension_roundtrip(key in proptest::collection::vec(0u8..16, 1..16)) {
            let leaf = RlpNode::from_rlp(&alloy_rlp::encode(LeafNode::new(Nibbles::new(), vec![1])));
            let branch = BranchNodeV2::new(Nibbles::from_nibbles(key), vec![leaf.clone(), leaf], TrieMask::from(3));
            assert!(branch.branch_rlp_node().unwrap().len() < 32);
            assert_roundtrip_and_length(TrieNodeV2::Branch(branch));
        }
    }

    #[test]
    #[should_panic(expected = "branch stack must match state mask")]
    fn rejects_inconsistent_branch_stack() {
        BranchNodeV2::new(Nibbles::new(), Vec::new(), TrieMask::from(1));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn rejects_inconsistent_deserialized_branch() {
        let mut value = serde_json::to_value(BranchNodeV2::default()).unwrap();
        value["state_mask"] = serde_json::to_value(TrieMask::from(1)).unwrap();
        assert!(serde_json::from_value::<BranchNodeV2>(value).is_err());
    }

    #[test]
    fn trie_node_variants_rlp_roundtrip_and_length() {
        assert_roundtrip_and_length(TrieNodeV2::EmptyRoot);
        assert_roundtrip_and_length(TrieNodeV2::Branch(BranchNodeV2::default()));
        assert_roundtrip_and_length(TrieNodeV2::Extension(ExtensionNode::new(
            Nibbles::from_nibbles([1]),
            RlpNode::word_rlp(&B256::repeat_byte(0xaa)),
        )));
    }

    proptest! {
        #[test]
        fn leaf_rlp_roundtrip_and_length(
            key in proptest::collection::vec(0u8..16, 0..64),
            value in proptest::collection::vec(any::<u8>(), 0..128),
        ) {
            let node = TrieNodeV2::Leaf(LeafNode::new(Nibbles::from_nibbles(key), value));
            assert_roundtrip_and_length(node);
        }
    }
}
