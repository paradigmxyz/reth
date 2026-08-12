//! Merkle Patricia trie range-proof verification.
//!
//! Reconstructs a trie root from consecutive hashed leaves and boundary proof nodes, rejecting
//! altered or incomplete ranges and reporting where the trie continues past the range.

use crate::{HashBuilder, Nibbles, RlpNode, TrieNode, EMPTY_ROOT_HASH};
use alloc::vec::Vec;
use alloy_primitives::{keccak256, map::B256Map, Bytes, B256};
use alloy_rlp::Decodable;

const KEY_NIBBLES: usize = B256::len_bytes() * 2;
const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

struct RangeProofVerifier<'a> {
    range: ProofRange,
    nodes: ProofNodes<'a>,
    frontier: ProofFrontier,
    next: Option<Nibbles>,
}

impl<'a> RangeProofVerifier<'a> {
    fn new(left: B256, right: B256, proof: &'a [Bytes], frontier: ProofFrontier) -> Self {
        Self {
            range: ProofRange::new(left, right),
            nodes: ProofNodes::new(proof),
            frontier,
            next: None,
        }
    }

    fn verify(mut self, root: B256) -> Result<Option<B256>, RangeProofError> {
        self.visit_reference(Nibbles::new(), &RlpNode::word_rlp(&root))?;

        let got = self.frontier.root()?;
        if got != root {
            return Err(RangeProofError::RootMismatch { expected: root, got })
        }
        Ok(self.next.as_ref().map(TriePath::lowest_key))
    }

    fn visit_reference(
        &mut self,
        prefix: Nibbles,
        reference: &RlpNode,
    ) -> Result<(), RangeProofError> {
        match self.range.subtree_relation(&prefix)? {
            SubtreeRelation::OutsideLeft => self.add_outside_reference(prefix, reference),
            SubtreeRelation::OutsideRight => {
                self.note_next(prefix);
                self.add_outside_reference(prefix, reference)
            }
            SubtreeRelation::Inside => Ok(()),
            SubtreeRelation::Boundary => {
                let node = self.nodes.resolve(prefix, reference)?;
                self.visit_node(node, prefix)
            }
        }
    }

    fn visit_node(&mut self, node: TrieNode, prefix: Nibbles) -> Result<(), RangeProofError> {
        match node {
            TrieNode::EmptyRoot => Ok(()),
            TrieNode::Leaf(leaf) => {
                let path = prefix.descend_leaf(&leaf.key)?;
                match self.range.key_relation(&path) {
                    KeyRelation::Before => self.frontier.push_leaf(path, leaf.value),
                    KeyRelation::Inside => {}
                    KeyRelation::After => {
                        self.note_next(path);
                        self.frontier.push_leaf(path, leaf.value);
                    }
                }
                Ok(())
            }
            TrieNode::Extension(extension) => {
                self.visit_reference(prefix.descend_extension(&extension.key)?, &extension.child)
            }
            TrieNode::Branch(branch) => {
                for (nibble, child) in branch
                    .as_ref()
                    .children()
                    .filter_map(|(nibble, child)| child.map(|child| (nibble, child)))
                {
                    self.visit_reference(prefix.descend_child(nibble)?, child)?;
                }
                Ok(())
            }
        }
    }

    fn add_outside_reference(
        &mut self,
        prefix: Nibbles,
        reference: &RlpNode,
    ) -> Result<(), RangeProofError> {
        if let Some(hash) = reference.as_hash() {
            self.frontier.push_subtree(prefix, hash);
            return Ok(())
        }
        self.add_outside_node(TrieNode::decode(&mut reference.as_slice())?, prefix)
    }

    fn add_outside_node(&mut self, node: TrieNode, prefix: Nibbles) -> Result<(), RangeProofError> {
        match node {
            TrieNode::EmptyRoot => Ok(()),
            TrieNode::Leaf(leaf) => {
                let path = prefix.descend_leaf(&leaf.key)?;
                self.frontier.push_leaf(path, leaf.value);
                Ok(())
            }
            TrieNode::Extension(extension) => self
                .add_outside_reference(prefix.descend_extension(&extension.key)?, &extension.child),
            TrieNode::Branch(branch) => {
                for (nibble, child) in branch
                    .as_ref()
                    .children()
                    .filter_map(|(nibble, child)| child.map(|child| (nibble, child)))
                {
                    self.add_outside_reference(prefix.descend_child(nibble)?, child)?;
                }
                Ok(())
            }
        }
    }

    fn note_next(&mut self, path: Nibbles) {
        if self.next.is_none_or(|next| path < next) {
            self.next = Some(path);
        }
    }
}

struct ProofRange {
    left: Nibbles,
    right: Nibbles,
}

impl ProofRange {
    fn new(left: B256, right: B256) -> Self {
        Self { left: Nibbles::unpack(left), right: Nibbles::unpack(right) }
    }

    fn subtree_relation(&self, prefix: &Nibbles) -> Result<SubtreeRelation, RangeProofError> {
        if prefix.len() > KEY_NIBBLES {
            return Err(RangeProofError::PathTooLong { path: *prefix })
        }
        let left = self.left.slice(..prefix.len());
        let right = self.right.slice(..prefix.len());

        Ok(if *prefix < left {
            SubtreeRelation::OutsideLeft
        } else if *prefix > right {
            SubtreeRelation::OutsideRight
        } else if *prefix > left && *prefix < right {
            SubtreeRelation::Inside
        } else {
            SubtreeRelation::Boundary
        })
    }

    fn key_relation(&self, path: &Nibbles) -> KeyRelation {
        if path < &self.left {
            KeyRelation::Before
        } else if path > &self.right {
            KeyRelation::After
        } else {
            KeyRelation::Inside
        }
    }
}

struct ProofNodes<'a>(B256Map<&'a [u8]>);

impl<'a> ProofNodes<'a> {
    fn new(proof: &'a [Bytes]) -> Self {
        Self(proof.iter().map(|node| (keccak256(node), node.as_ref())).collect())
    }

    fn resolve(&self, path: Nibbles, reference: &RlpNode) -> Result<TrieNode, RangeProofError> {
        let Some(hash) = reference.as_hash() else {
            return Ok(TrieNode::decode(&mut reference.as_slice())?)
        };
        let node = self.0.get(&hash).ok_or(RangeProofError::MissingProofNode { path })?;
        Ok(TrieNode::decode(&mut &node[..])?)
    }
}

#[derive(Default)]
struct ProofFrontier(Vec<FrontierEntry>);

impl ProofFrontier {
    fn from_leaves<I, V>(origin: B256, leaves: I) -> Result<(Self, Option<B256>), RangeProofError>
    where
        I: IntoIterator<Item = (B256, V)>,
        V: Into<Vec<u8>>,
    {
        let mut frontier = Self::default();
        let mut previous = None;

        for (key, value) in leaves {
            let value = value.into();
            if key < origin {
                return Err(RangeProofError::LeafBeforeOrigin { key, origin })
            }
            if previous.is_some_and(|previous| key <= previous) {
                return Err(RangeProofError::NonMonotonicLeaves)
            }
            if value.is_empty() {
                return Err(RangeProofError::EmptyLeafValue { key })
            }
            previous = Some(key);
            frontier.push_leaf(Nibbles::unpack(key), value);
        }

        Ok((frontier, previous))
    }

    fn push_leaf(&mut self, path: Nibbles, value: Vec<u8>) {
        debug_assert_eq!(path.len(), KEY_NIBBLES);
        self.0.push(FrontierEntry::Leaf { path, value });
    }

    fn push_subtree(&mut self, path: Nibbles, hash: B256) {
        debug_assert!(path.len() <= KEY_NIBBLES);
        self.0.push(FrontierEntry::Subtree { path, hash });
    }

    fn root(mut self) -> Result<B256, RangeProofError> {
        // Outside subtries are disjoint from returned leaves, so sorting produces the strict path
        // order required by HashBuilder. Reject duplicates before they reach its assertion.
        self.0.sort_unstable_by_key(FrontierEntry::path);
        let mut builder = HashBuilder::default();
        let mut previous = None;

        for entry in self.0 {
            let path = entry.path();
            if previous.is_some_and(|previous| path <= previous) {
                return Err(RangeProofError::DuplicateFrontierPath { path })
            }
            previous = Some(path);
            match entry {
                FrontierEntry::Leaf { path, value } => builder.add_leaf(path, &value),
                FrontierEntry::Subtree { path, hash } => builder.add_branch(path, hash, false),
            }
        }
        Ok(builder.root())
    }
}

/// Error returned when a trie range proof is invalid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RangeProofError {
    /// The response leaves are not strictly increasing.
    #[error("range leaves are not strictly increasing")]
    NonMonotonicLeaves,
    /// A returned leaf precedes the requested origin.
    #[error("range leaf {key} precedes origin {origin}")]
    LeafBeforeOrigin {
        /// Hashed key of the offending leaf.
        key: B256,
        /// Inclusive origin the range was requested from.
        origin: B256,
    },
    /// A returned leaf has no value and would represent a deletion.
    #[error("range leaf {key} has an empty value")]
    EmptyLeafValue {
        /// Hashed key of the valueless leaf.
        key: B256,
    },
    /// A proof node required on a range boundary is missing.
    #[error("missing proof node at path {path:?}")]
    MissingProofNode {
        /// Trie path the missing node was referenced from.
        path: Nibbles,
    },
    /// An extension node consumes no nibble, so a crafted chain could recurse without descending.
    #[error("extension node at path {path:?} has an empty key")]
    EmptyExtensionKey {
        /// Trie path the extension was reached at.
        path: Nibbles,
    },
    /// A proof path exceeds a hashed key's fixed length.
    #[error("proof path {path:?} exceeds hashed key length")]
    PathTooLong {
        /// Trie path that could not be descended any further.
        path: Nibbles,
    },
    /// A leaf does not resolve to a complete hashed key.
    #[error("leaf path {path:?} does not resolve to a hashed key")]
    InvalidLeafPath {
        /// Incomplete trie path the leaf terminated at.
        path: Nibbles,
    },
    /// Two reconstructed trie entries occupy the same path.
    #[error("duplicate range-proof frontier path {path:?}")]
    DuplicateFrontierPath {
        /// Trie path claimed by more than one entry.
        path: Nibbles,
    },
    /// The reconstructed trie does not match the requested root.
    #[error("range proof root mismatch: expected {expected}, got {got}")]
    RootMismatch {
        /// Root the range was requested against.
        expected: B256,
        /// Root reconstructed from the response.
        got: B256,
    },
    /// A trie node failed to decode.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}

/// Verifies a consecutive leaf range against `root`, starting at `origin`.
///
/// Returns a lower bound for the next key, or `None` if the range exhausts the trie.
pub fn verify_range_proof<I, V>(
    root: B256,
    origin: B256,
    leaves: I,
    proof: &[Bytes],
) -> Result<Option<B256>, RangeProofError>
where
    I: IntoIterator<Item = (B256, V)>,
    V: Into<Vec<u8>>,
{
    let (frontier, last_key) = ProofFrontier::from_leaves(origin, leaves)?;

    // Empty tries and proof-free ranges are authenticated by their reconstructed root.
    if root == EMPTY_ROOT_HASH || proof.is_empty() {
        let got = frontier.root()?;
        if got != root {
            return Err(RangeProofError::RootMismatch { expected: root, got })
        }
        return Ok(None)
    }

    RangeProofVerifier::new(origin, last_key.unwrap_or(MAX_HASH), proof, frontier).verify(root)
}

trait TriePath: Sized {
    fn lowest_key(&self) -> B256;

    fn descend_extension(self, key: &Nibbles) -> Result<Self, RangeProofError>;

    fn descend_child(self, nibble: u8) -> Result<Self, RangeProofError>;

    fn descend_leaf(self, key: &Nibbles) -> Result<Self, RangeProofError>;

    fn join_checked(self, key: &Nibbles) -> Result<Self, RangeProofError>;
}

impl TriePath for Nibbles {
    // Packing zero-fills the nibbles the path leaves free, which is the key it bounds.
    fn lowest_key(&self) -> B256 {
        B256::right_padding_from(&self.pack())
    }

    fn descend_extension(self, key: &Nibbles) -> Result<Self, RangeProofError> {
        // A canonical extension always consumes a nibble. An empty key would let a crafted chain of
        // extensions recurse at a fixed depth until the stack is exhausted.
        if key.is_empty() {
            return Err(RangeProofError::EmptyExtensionKey { path: self })
        }
        self.join_checked(key)
    }

    fn descend_child(self, nibble: u8) -> Result<Self, RangeProofError> {
        if self.len() >= KEY_NIBBLES {
            return Err(RangeProofError::PathTooLong { path: self })
        }
        let mut path = self;
        path.push(nibble);
        Ok(path)
    }

    fn descend_leaf(self, key: &Nibbles) -> Result<Self, RangeProofError> {
        let path = self.join_checked(key)?;
        if path.len() != KEY_NIBBLES {
            return Err(RangeProofError::InvalidLeafPath { path })
        }
        Ok(path)
    }

    fn join_checked(self, key: &Nibbles) -> Result<Self, RangeProofError> {
        if self.len() + key.len() > KEY_NIBBLES {
            return Err(RangeProofError::PathTooLong { path: self })
        }
        Ok(self.join(key))
    }
}

#[derive(Clone, Debug)]
enum FrontierEntry {
    Leaf { path: Nibbles, value: Vec<u8> },
    Subtree { path: Nibbles, hash: B256 },
}

impl FrontierEntry {
    const fn path(&self) -> Nibbles {
        match self {
            Self::Leaf { path, .. } | Self::Subtree { path, .. } => *path,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubtreeRelation {
    OutsideLeft,
    OutsideRight,
    Boundary,
    Inside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRelation {
    Before,
    Inside,
    After,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{proof::ProofRetainer, BranchNode, ExtensionNode, TrieMask};
    use alloc::{vec, vec::Vec};

    fn key(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    fn encode_node(node: &TrieNode) -> Bytes {
        alloy_rlp::encode(node).into()
    }

    fn no_leaves() -> Vec<(B256, Vec<u8>)> {
        Vec::new()
    }

    fn value(byte: u8) -> Vec<u8> {
        vec![byte; 64]
    }

    fn build_proof(leaves: &[(B256, Vec<u8>)], targets: &[B256]) -> (B256, Vec<Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (key, value) in leaves {
            builder.add_leaf(Nibbles::unpack(*key), value);
        }
        let root = builder.root();
        let proof = builder
            .take_proof_nodes()
            .into_nodes_sorted()
            .into_iter()
            .map(|(_, node)| node)
            .collect();
        (root, proof)
    }

    #[test]
    fn partial_range_authenticates_and_reports_the_next_key() {
        let leaves =
            vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3)), (key(4), value(4))];
        let (root, proof) = build_proof(&leaves, &[key(2), key(3)]);

        assert_eq!(
            verify_range_proof(root, key(2), leaves[1..3].to_vec(), &proof).unwrap(),
            Some(key(4))
        );
    }

    // An unexpanded subtree reports its lowest possible key.
    #[test]
    fn unexpanded_right_subtree_reports_a_prefix_bound() {
        let right = |tail: u8| {
            let mut key = B256::ZERO;
            key.0[0] = 0x40;
            key.0[31] = tail;
            key
        };
        let leaves = vec![
            (key(1), value(1)),
            (key(2), value(2)),
            (right(1), value(3)),
            (right(2), value(4)),
        ];
        let (root, proof) = build_proof(&leaves, &[key(1), key(2)]);

        assert_eq!(
            verify_range_proof(root, key(1), leaves[..2].to_vec(), &proof).unwrap(),
            Some(B256::right_padding_from(&[0x40]))
        );
    }

    #[test]
    fn inline_outside_nodes_are_reconstructed() {
        let leaves =
            vec![(key(1), vec![1]), (key(2), vec![2]), (key(3), vec![3]), (key(4), vec![4])];
        let (root, proof) = build_proof(&leaves, &[key(2), key(3)]);

        assert_eq!(
            verify_range_proof(root, key(2), leaves[1..3].to_vec(), &proof).unwrap(),
            Some(key(4))
        );
    }

    #[test]
    fn unused_proof_nodes_are_accepted() {
        let leaves =
            vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3)), (key(4), value(4))];
        let (root, mut proof) = build_proof(&leaves, &[key(2), key(3)]);
        proof.push(Bytes::from_static(&[alloy_rlp::EMPTY_STRING_CODE]));

        assert_eq!(
            verify_range_proof(root, key(2), leaves[1..3].to_vec(), &proof).unwrap(),
            Some(key(4))
        );
    }

    // Reject paths before they exceed the fixed hashed-key length.
    #[test]
    fn node_paths_are_bounded_before_they_overflow_a_key() {
        let dangling = RlpNode::word_rlp(&B256::repeat_byte(0xaa));

        // An extension one nibble deep whose key spans a whole hashed key: 1 + 64 nibbles.
        let overlong = encode_node(&TrieNode::Extension(ExtensionNode::new(
            Nibbles::unpack(key(1)),
            dangling.clone(),
        )));
        let branch = encode_node(&TrieNode::Branch(BranchNode::new(
            vec![RlpNode::word_rlp(&keccak256(&overlong))],
            TrieMask::new(1),
        )));
        let proof = vec![branch.clone(), overlong];

        assert!(matches!(
            verify_range_proof(keccak256(&branch), key(1), no_leaves(), &proof),
            Err(RangeProofError::PathTooLong { .. })
        ));

        // A branch sitting at the full hashed-key depth, which has no room for a child.
        let deep_branch =
            encode_node(&TrieNode::Branch(BranchNode::new(vec![dangling], TrieMask::new(1))));
        let reach = encode_node(&TrieNode::Extension(ExtensionNode::new(
            Nibbles::unpack(key(1)),
            RlpNode::word_rlp(&keccak256(&deep_branch)),
        )));
        let proof = vec![reach.clone(), deep_branch];

        assert!(matches!(
            verify_range_proof(keccak256(&reach), key(1), no_leaves(), &proof),
            Err(RangeProofError::PathTooLong { .. })
        ));
    }

    // Empty extensions could recurse indefinitely without increasing the path depth.
    #[test]
    fn empty_extension_keys_are_rejected() {
        let empty_key = Nibbles::new();
        let mut proof = Vec::new();
        let mut child = RlpNode::word_rlp(&B256::repeat_byte(0xaa));

        for _ in 0..64 {
            let node = encode_node(&TrieNode::Extension(ExtensionNode::new(empty_key, child)));
            child = RlpNode::word_rlp(&keccak256(&node));
            proof.push(node);
        }
        let root = keccak256(proof.last().unwrap());

        assert!(matches!(
            verify_range_proof(root, key(1), no_leaves(), &proof),
            Err(RangeProofError::EmptyExtensionKey { .. })
        ));
    }

    #[test]
    fn missing_boundary_node_is_rejected() {
        let leaves =
            vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3)), (key(4), value(4))];
        let (root, _) = build_proof(&leaves, &[key(2), key(3)]);
        let unrelated = [Bytes::from_static(&[alloy_rlp::EMPTY_STRING_CODE])];

        assert!(matches!(
            verify_range_proof(root, key(2), leaves[1..3].to_vec(), &unrelated),
            Err(RangeProofError::MissingProofNode { .. })
        ));
    }

    #[test]
    fn boundary_proof_can_authenticate_an_exhausted_range() {
        let leaves =
            vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3)), (key(4), value(4))];
        let (root, proof) = build_proof(&leaves, &[key(2), key(4)]);

        assert_eq!(verify_range_proof(root, key(2), leaves[1..].to_vec(), &proof).unwrap(), None);
    }

    #[test]
    fn proof_free_full_trie_is_exhausted() {
        let leaves = vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3))];
        let (root, _) = build_proof(&leaves, &[]);

        assert_eq!(verify_range_proof(root, B256::ZERO, leaves, &[]).unwrap(), None);
    }

    #[test]
    fn omitted_interior_leaf_changes_root() {
        let leaves =
            vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3)), (key(4), value(4))];
        let (root, proof) = build_proof(&leaves, &[key(2), key(4)]);
        let returned = vec![(key(2), value(2)), (key(4), value(4))];

        assert!(matches!(
            verify_range_proof(root, key(2), returned, &proof),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }

    #[test]
    fn mutated_leaf_changes_root() {
        let leaves = vec![(key(1), value(1)), (key(2), value(2)), (key(3), value(3))];
        let (root, proof) = build_proof(&leaves, &[key(2), key(3)]);
        let returned = vec![(key(2), value(9)), (key(3), value(3))];

        assert!(matches!(
            verify_range_proof(root, key(2), returned, &proof),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }

    #[test]
    fn empty_tail_is_authenticated() {
        let leaves = vec![(key(1), value(1)), (key(2), value(2))];
        let (root, proof) = build_proof(&leaves, &[key(3)]);

        assert_eq!(
            verify_range_proof(root, key(3), core::iter::empty::<(B256, Vec<u8>)>(), &proof)
                .unwrap(),
            None
        );
    }

    #[test]
    fn empty_range_cannot_hide_a_right_leaf() {
        let leaves = vec![(key(1), value(1)), (key(3), value(3))];
        let (root, proof) = build_proof(&leaves, &[key(2)]);

        assert!(matches!(
            verify_range_proof(root, key(2), core::iter::empty::<(B256, Vec<u8>)>(), &proof,),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }

    #[test]
    fn rejects_non_monotonic_leaves_and_leaves_before_origin() {
        let leaves = vec![(key(2), value(2)), (key(1), value(1))];

        assert_eq!(
            verify_range_proof(B256::ZERO, B256::ZERO, leaves, &[]),
            Err(RangeProofError::NonMonotonicLeaves)
        );
        assert!(matches!(
            verify_range_proof(B256::ZERO, key(2), [(key(1), value(1))], &[],),
            Err(RangeProofError::LeafBeforeOrigin { .. })
        ));
        assert_eq!(
            verify_range_proof(B256::ZERO, B256::ZERO, [(key(1), Vec::new())], &[]),
            Err(RangeProofError::EmptyLeafValue { key: key(1) })
        );
    }

    #[test]
    fn empty_root_accepts_only_an_empty_range() {
        assert_eq!(
            verify_range_proof(
                EMPTY_ROOT_HASH,
                B256::ZERO,
                core::iter::empty::<(B256, Vec<u8>)>(),
                &[]
            )
            .unwrap(),
            None
        );
        assert!(matches!(
            verify_range_proof(EMPTY_ROOT_HASH, B256::ZERO, [(key(1), value(1))], &[],),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }
}
