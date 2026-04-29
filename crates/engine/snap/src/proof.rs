//! Snap range proof verification.
//!
//! Snap range responses prove a consecutive leaf range with the boundary trie
//! nodes that connect the returned leaves to the rest of the trie. Verifying
//! only the first and last leaf is not enough: an adversarial peer could omit a
//! leaf in the middle while still proving both endpoints. The verifier below
//! reconstructs the trie root from returned leaves plus proof subtrees that are
//! outside the proven range.

use alloy_primitives::{Bytes, B256};
use alloy_rlp::Decodable;
use reth_trie::{HashBuilder, Nibbles, RlpNode, TrieNode, EMPTY_ROOT_HASH};
use std::collections::HashMap;

const KEY_NIBBLES: usize = 64;
const MAX_HASH: B256 = B256::new([0xff; 32]);

/// Error returned when a snap range proof is invalid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum RangeProofError {
    /// The response leaves were not strictly increasing by hashed key.
    #[error("range leaves are not strictly increasing")]
    NonMonotonicLeaves,
    /// A returned leaf is before the requested range origin.
    #[error("range leaf {key} is before origin {origin}")]
    LeafBeforeOrigin {
        /// The invalid leaf key.
        key: B256,
        /// Requested range origin.
        origin: B256,
    },
    /// A proof node needed to reconstruct the trie boundary was missing.
    #[error("missing proof node at path {path:?}")]
    MissingProofNode {
        /// Trie path whose node reference was required.
        path: Nibbles,
    },
    /// A decoded proof path exceeded the fixed 32-byte hashed-key length.
    #[error("proof path {path:?} exceeds hashed key length")]
    PathTooLong {
        /// Invalid trie path.
        path: Nibbles,
    },
    /// A leaf proof path did not resolve to a full 32-byte hashed key.
    #[error("leaf proof path {path:?} does not resolve to a full hashed key")]
    InvalidLeafPath {
        /// Invalid trie path.
        path: Nibbles,
    },
    /// The reconstructed frontier contained duplicate paths.
    #[error("range proof frontier contains duplicate path {path:?}")]
    DuplicateFrontierPath {
        /// Duplicate trie path.
        path: Nibbles,
    },
    /// The reconstructed root does not match the expected trie root.
    #[error("range proof root mismatch: expected {expected}, got {got}")]
    RootMismatch {
        /// Expected trie root.
        expected: B256,
        /// Root reconstructed from leaves and proof frontier.
        got: B256,
    },
    /// A trie node failed to decode.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}

/// Verifies that `leaves` are complete from `origin` through the last returned
/// leaf, or through the end of the trie when no leaves were returned.
pub(crate) fn verify_range_proof<I, V>(
    root: B256,
    origin: B256,
    leaves: I,
    proof: &[Bytes],
) -> Result<(), RangeProofError>
where
    I: IntoIterator<Item = (B256, V)>,
    V: AsRef<[u8]>,
{
    let mut frontier = Vec::new();
    let mut previous = None;
    let mut last_key = None;

    for (key, value) in leaves {
        if key < origin {
            return Err(RangeProofError::LeafBeforeOrigin { key, origin })
        }
        if previous.is_some_and(|previous| key <= previous) {
            return Err(RangeProofError::NonMonotonicLeaves)
        }

        previous = Some(key);
        last_key = Some(key);
        frontier.push(FrontierEntry::Leaf {
            path: Nibbles::unpack(key),
            value: value.as_ref().to_vec(),
        });
    }

    if root == EMPTY_ROOT_HASH {
        if frontier.is_empty() {
            return Ok(())
        }
        return Err(RangeProofError::RootMismatch { expected: root, got: frontier_root(frontier)? })
    }

    if !proof.is_empty() && !proof_is_empty_root(proof) {
        let proof_by_reference = proof
            .iter()
            .map(|node| (RlpNode::from_rlp(node).as_slice().to_vec(), node.as_ref()))
            .collect::<HashMap<_, _>>();

        let left = Nibbles::unpack(origin);
        let right = Nibbles::unpack(last_key.unwrap_or(MAX_HASH));
        visit_reference(
            Nibbles::new(),
            &RlpNode::word_rlp(&root),
            &left,
            &right,
            &proof_by_reference,
            &mut frontier,
        )?;
    }

    let got = frontier_root(frontier)?;
    if got != root {
        return Err(RangeProofError::RootMismatch { expected: root, got })
    }

    Ok(())
}

fn proof_is_empty_root(proof: &[Bytes]) -> bool {
    proof.len() == 1 && proof[0].as_ref() == [alloy_rlp::EMPTY_STRING_CODE]
}

fn visit_reference(
    prefix: Nibbles,
    reference: &RlpNode,
    left: &Nibbles,
    right: &Nibbles,
    proof_by_reference: &HashMap<Vec<u8>, &[u8]>,
    frontier: &mut Vec<FrontierEntry>,
) -> Result<(), RangeProofError> {
    match subtree_relation(&prefix, left, right)? {
        SubtreeRelation::Outside => add_outside_reference(prefix, reference, frontier),
        SubtreeRelation::Inside => Ok(()),
        SubtreeRelation::Boundary => {
            let node = resolve_reference(prefix, reference, proof_by_reference)?;
            visit_node(node, prefix, left, right, proof_by_reference, frontier)
        }
    }
}

fn visit_node(
    node: TrieNode,
    prefix: Nibbles,
    left: &Nibbles,
    right: &Nibbles,
    proof_by_reference: &HashMap<Vec<u8>, &[u8]>,
    frontier: &mut Vec<FrontierEntry>,
) -> Result<(), RangeProofError> {
    match node {
        TrieNode::EmptyRoot => Ok(()),
        TrieNode::Leaf(leaf) => {
            let path = join_path(prefix, &leaf.key)?;
            if path.len() != KEY_NIBBLES {
                return Err(RangeProofError::InvalidLeafPath { path })
            }
            if key_in_range(&path, left, right) {
                Ok(())
            } else {
                frontier.push(FrontierEntry::Leaf { path, value: leaf.value });
                Ok(())
            }
        }
        TrieNode::Extension(extension) => visit_reference(
            join_path(prefix, &extension.key)?,
            &extension.child,
            left,
            right,
            proof_by_reference,
            frontier,
        ),
        TrieNode::Branch(branch) => {
            for (nibble, child) in branch
                .as_ref()
                .children()
                .filter_map(|(nibble, child)| child.map(|child| (nibble, child)))
            {
                let mut child_prefix = prefix;
                child_prefix.push(nibble);
                visit_reference(child_prefix, child, left, right, proof_by_reference, frontier)?;
            }
            Ok(())
        }
    }
}

fn add_outside_reference(
    prefix: Nibbles,
    reference: &RlpNode,
    frontier: &mut Vec<FrontierEntry>,
) -> Result<(), RangeProofError> {
    if prefix.len() > KEY_NIBBLES {
        return Err(RangeProofError::PathTooLong { path: prefix })
    }

    if let Some(hash) = reference.as_hash() {
        frontier.push(FrontierEntry::Subtree { path: prefix, hash });
        return Ok(())
    }

    add_outside_node(TrieNode::decode(&mut reference.as_slice())?, prefix, frontier)
}

fn add_outside_node(
    node: TrieNode,
    prefix: Nibbles,
    frontier: &mut Vec<FrontierEntry>,
) -> Result<(), RangeProofError> {
    match node {
        TrieNode::EmptyRoot => Ok(()),
        TrieNode::Leaf(leaf) => {
            let path = join_path(prefix, &leaf.key)?;
            if path.len() != KEY_NIBBLES {
                return Err(RangeProofError::InvalidLeafPath { path })
            }
            frontier.push(FrontierEntry::Leaf { path, value: leaf.value });
            Ok(())
        }
        TrieNode::Extension(extension) => {
            add_outside_reference(join_path(prefix, &extension.key)?, &extension.child, frontier)
        }
        TrieNode::Branch(branch) => {
            for (nibble, child) in branch
                .as_ref()
                .children()
                .filter_map(|(nibble, child)| child.map(|child| (nibble, child)))
            {
                let mut child_prefix = prefix;
                child_prefix.push(nibble);
                add_outside_reference(child_prefix, child, frontier)?;
            }
            Ok(())
        }
    }
}

fn resolve_reference(
    path: Nibbles,
    reference: &RlpNode,
    proof_by_reference: &HashMap<Vec<u8>, &[u8]>,
) -> Result<TrieNode, RangeProofError> {
    if !reference.is_hash() {
        return Ok(TrieNode::decode(&mut reference.as_slice())?)
    }

    let Some(node) = proof_by_reference.get(reference.as_slice()) else {
        return Err(RangeProofError::MissingProofNode { path })
    };
    Ok(TrieNode::decode(&mut &node[..])?)
}

fn join_path(mut prefix: Nibbles, suffix: &Nibbles) -> Result<Nibbles, RangeProofError> {
    prefix.extend(suffix);
    if prefix.len() > KEY_NIBBLES {
        return Err(RangeProofError::PathTooLong { path: prefix })
    }
    Ok(prefix)
}

fn frontier_root(mut frontier: Vec<FrontierEntry>) -> Result<B256, RangeProofError> {
    frontier.sort_unstable_by_key(|entry| entry.path());

    let mut builder = HashBuilder::default();
    let mut previous = None;
    for entry in frontier {
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

fn subtree_relation(
    prefix: &Nibbles,
    left: &Nibbles,
    right: &Nibbles,
) -> Result<SubtreeRelation, RangeProofError> {
    if prefix.len() > KEY_NIBBLES {
        return Err(RangeProofError::PathTooLong { path: *prefix })
    }

    let min = padded_path(prefix, 0);
    let max = padded_path(prefix, 0x0f);
    let left = padded_path(left, 0);
    let right = padded_path(right, 0x0f);

    if max < left || min > right {
        Ok(SubtreeRelation::Outside)
    } else if min >= left && max <= right {
        Ok(SubtreeRelation::Inside)
    } else {
        Ok(SubtreeRelation::Boundary)
    }
}

fn key_in_range(key: &Nibbles, left: &Nibbles, right: &Nibbles) -> bool {
    let key = padded_path(key, 0);
    key >= padded_path(left, 0) && key <= padded_path(right, 0x0f)
}

fn padded_path(path: &Nibbles, fill: u8) -> [u8; KEY_NIBBLES] {
    let mut padded = [fill; KEY_NIBBLES];
    for (idx, nibble) in padded.iter_mut().enumerate().take(path.len()) {
        *nibble = path.get(idx).expect("idx is below path length");
    }
    padded
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
    Outside,
    Boundary,
    Inside,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_trie::proof::ProofRetainer;
    use reth_trie::HashBuilder;

    fn b256(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
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
    fn complete_range_accepts_boundary_multiproof() {
        let leaves = vec![
            (b256(1), value(1)),
            (b256(2), value(2)),
            (b256(3), value(3)),
            (b256(4), value(4)),
        ];
        let returned = leaves[1..=3].to_vec();
        let (root, proof) = build_proof(&leaves, &[b256(2), b256(4)]);

        verify_range_proof(root, b256(2), returned, &proof).unwrap();
    }

    #[test]
    fn proof_free_full_range_verifies_from_leaves() {
        let leaves = vec![(b256(1), value(1)), (b256(2), value(2)), (b256(3), value(3))];
        let (root, _) = build_proof(&leaves, &[]);

        verify_range_proof(root, B256::ZERO, leaves, &[]).unwrap();
    }

    #[test]
    fn range_rejects_omitted_interior_leaf() {
        let leaves = vec![
            (b256(1), value(1)),
            (b256(2), value(2)),
            (b256(3), value(3)),
            (b256(4), value(4)),
        ];
        let returned = vec![(b256(2), value(2)), (b256(4), value(4))];
        let (root, proof) = build_proof(&leaves, &[b256(2), b256(4)]);

        assert!(matches!(
            verify_range_proof(root, b256(2), returned, &proof),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }

    #[test]
    fn empty_tail_range_accepts_absence_proof() {
        let leaves = vec![(b256(1), value(1)), (b256(2), value(2))];
        let (root, proof) = build_proof(&leaves, &[b256(3)]);

        verify_range_proof(root, b256(3), std::iter::empty::<(B256, Vec<u8>)>(), &proof).unwrap();
    }

    #[test]
    fn empty_range_rejects_omitted_right_leaf() {
        let leaves = vec![(b256(1), value(1)), (b256(3), value(3))];
        let (root, proof) = build_proof(&leaves, &[b256(2)]);

        assert!(matches!(
            verify_range_proof(root, b256(2), std::iter::empty::<(B256, Vec<u8>)>(), &proof),
            Err(RangeProofError::RootMismatch { .. })
        ));
    }
}
