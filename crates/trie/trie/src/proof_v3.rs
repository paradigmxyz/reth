//! Bottom-up proofs over complete persisted state tries.
use crate::state_trie_cursor::{StateTrieCursor, StateTrieStorageCursor};
use alloy_primitives::{keccak256, B256};
use alloy_rlp::Encodable;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{depth_first_cmp, ProofTrieNodeV2, ProofV2Target, StateTrieNode};
use std::collections::BTreeMap;

/// Reusable proof calculator. Only the initial neighbor lookup needs an ordered seek;
/// each ancestor is fetched by its exact, derived key.
#[derive(Debug)]
pub struct ProofCalculator<C> {
    cursor: C,
    rlp_buf: Vec<u8>,
}

impl<C: StateTrieCursor> ProofCalculator<C>
where
    C::Value: Encodable,
{
    /// Create a calculator over the complete trie.
    pub const fn new(cursor: C) -> Self {
        Self { cursor, rlp_buf: Vec::new() }
    }

    /// Return proof nodes in children-before-parents order, respecting known-parent bounds.
    pub fn proof(
        &mut self,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        let mut proof = BTreeMap::new();
        for target in targets {
            let key = target.key_nibbles;
            let Some(mut node) = self.cursor.get(key)? else {
                // An absent leaf has no bottom-up starting point. Follow the stored subtree
                // from the known parent's child, stopping at the first absence witness.
                let mut prefix = key.slice(..target.parent.path_len().map_or(0, |len| len + 1));
                let mut required_child = false;
                loop {
                    let next = self.cursor.seek(prefix)?;
                    let Some((path, mut node)) = next.filter(|(path, _)| path.starts_with(&prefix))
                    else {
                        if required_child {
                            return Err(StateProofError::TrieInconsistency(
                                format!(
                                    "missing state trie child at {prefix:?} for target {key:?}, known parent {:?}",
                                    target.parent
                                ),
                            ))
                        }
                        if prefix.is_empty() {
                            proof.insert(prefix, ProofTrieNodeV2::empty());
                        }
                        break
                    };
                    let short_len = match &node {
                        StateTrieNode::Leaf { short_key_len, .. } |
                        StateTrieNode::Branch { short_key_len, .. } => *short_key_len as usize,
                    };
                    if short_len > path.len() || path.len() - short_len != prefix.len() {
                        if short_len <= path.len() && path.len() - short_len < prefix.len() {
                            let short_key_len = match &mut node {
                                StateTrieNode::Leaf { short_key_len, .. } |
                                StateTrieNode::Branch { short_key_len, .. } => short_key_len,
                            };
                            *short_key_len = (path.len() - prefix.len()) as u8;
                        } else {
                            return Err(StateProofError::TrieInconsistency(
                                format!(
                                    "missing state trie subtree root at {prefix:?}: next {path:?}, short key {short_len}, target {key:?}, known parent {:?}",
                                    target.parent
                                ),
                            ))
                        }
                    }
                    let child = match &node {
                        StateTrieNode::Branch { state_mask, .. } if key.starts_with(&path) => {
                            if path.len() >= key.len() {
                                return Err(StateProofError::TrieInconsistency(
                                    "branch at full leaf path".into(),
                                ))
                            }
                            let nibble = key.get_unchecked(path.len());
                            state_mask.is_bit_set(nibble).then(|| {
                                let mut child = path;
                                child.push_unchecked(nibble);
                                child
                            })
                        }
                        _ => None,
                    };
                    proof.entry(prefix).or_insert_with(|| node.proof_node(path));
                    let Some(child) = child else { break };
                    prefix = child;
                    required_child = true;
                }
                continue
            };
            let mut path = key;
            if let Some(parent_len) = target.parent.path_len() {
                let short_key_len = match &mut node {
                    StateTrieNode::Leaf { short_key_len, .. } |
                    StateTrieNode::Branch { short_key_len, .. } => short_key_len,
                };
                if *short_key_len as usize <= path.len() &&
                    path.len() - *short_key_len as usize <= parent_len
                {
                    *short_key_len = (path.len() - parent_len - 1) as u8;
                }
            }
            loop {
                let short_len = match &node {
                    StateTrieNode::Leaf { short_key_len, .. } |
                    StateTrieNode::Branch { short_key_len, .. } => *short_key_len as usize,
                };
                if short_len > path.len() {
                    return Err(StateProofError::TrieInconsistency(
                        "short key exceeds node path".into(),
                    ))
                }
                let physical = path.slice(..path.len() - short_len);
                if target.parent.path_len().is_some_and(|len| physical.len() <= len) {
                    break
                }
                if key.starts_with(&physical) {
                    proof.entry(physical).or_insert_with(|| node.proof_node(path));
                }
                if physical.is_empty() {
                    break
                }
                path = physical.slice(..physical.len() - 1);
                // Masked persistence may omit this ancestor because it is already retained in
                // the sparse trie. Stop before reading the known parent from the database.
                if target.parent.path_len().is_some_and(|len| path.len() <= len) {
                    break
                }
                node = self.cursor.get(path)?.ok_or_else(|| {
                    StateProofError::TrieInconsistency(format!(
                        "missing state trie parent at {path:?} for target {key:?}, known parent {:?}, child {physical:?}",
                        target.parent,
                    ))
                })?;
                if !matches!(node, StateTrieNode::Branch { .. }) {
                    return Err(StateProofError::TrieInconsistency("parent is not a branch".into()))
                }
            }
        }
        let mut proof: Vec<_> = proof.into_values().collect();
        proof.sort_unstable_by(|a, b| depth_first_cmp(&a.path, &b.path));
        Ok(proof)
    }

    /// Hash a complete proof's root, returning `None` for partial proofs.
    pub fn compute_root_hash(
        &mut self,
        nodes: &[ProofTrieNodeV2],
    ) -> Result<Option<B256>, StateProofError> {
        let Some(root) = nodes.iter().find(|n| n.path.is_empty()) else { return Ok(None) };
        self.rlp_buf.clear();
        root.node.encode(&mut self.rlp_buf);
        Ok(Some(keccak256(&self.rlp_buf)))
    }

    /// Retrieve the trie root without calculating unrelated subtries.
    pub fn root_node(&mut self) -> Result<ProofTrieNodeV2, StateProofError> {
        let mut proof = self.proof(&mut [ProofV2Target::new(B256::ZERO)])?;
        Ok(proof.pop().unwrap_or_else(ProofTrieNodeV2::empty))
    }
}

/// Storage proof calculator with reusable address selection.
pub type StorageProofCalculator<C> = ProofCalculator<C>;
impl<C: StateTrieStorageCursor> ProofCalculator<C> {
    /// Create a storage proof calculator.
    pub const fn new_storage(cursor: C) -> Self {
        Self::new(cursor)
    }
    /// Prove storage targets for an account.
    pub fn storage_proof(
        &mut self,
        address: B256,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        self.cursor.set_hashed_address(address);
        self.proof(targets)
    }
    /// Retrieve a storage trie's root node.
    pub fn storage_root_node(&mut self, address: B256) -> Result<ProofTrieNodeV2, StateProofError> {
        self.cursor.set_hashed_address(address);
        self.root_node()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_storage_errors::db::DatabaseError;
    use reth_trie_common::{Nibbles, ProofV2TargetParent, StateTrieBuilder};

    #[derive(Debug)]
    struct Cursor(BTreeMap<Nibbles, StateTrieNode<U256>>);
    impl StateTrieCursor for Cursor {
        type Value = U256;
        fn get(&mut self, p: Nibbles) -> Result<Option<StateTrieNode<U256>>, DatabaseError> {
            Ok(self.0.get(&p).cloned())
        }
        fn seek(
            &mut self,
            p: Nibbles,
        ) -> Result<Option<(Nibbles, StateTrieNode<U256>)>, DatabaseError> {
            Ok(self.0.range(p..).next().map(|(p, n)| (*p, n.clone())))
        }
        fn before(
            &mut self,
            p: Option<Nibbles>,
        ) -> Result<Option<(Nibbles, StateTrieNode<U256>)>, DatabaseError> {
            Ok(match p {
                Some(p) => self.0.range(..p).next_back(),
                None => self.0.last_key_value(),
            }
            .map(|(p, n)| (*p, n.clone())))
        }
    }

    #[test]
    fn partial_proof_does_not_read_masked_known_parent() {
        for parent_len in [0, 2, 62] {
            let key = B256::repeat_byte(0x11);
            let path = Nibbles::unpack(key);
            let leaf = StateTrieNode::Leaf {
                short_key_len: (63 - parent_len) as u8,
                value: U256::from(1),
            };
            let expected = leaf.clone().proof_node(path);
            let mut calculator = ProofCalculator::new(Cursor([(path, leaf)].into()));
            let mut absent = key;
            absent.0[31] = 0x12;
            for target in [key, absent] {
                let proof = calculator
                    .proof(&mut [ProofV2Target::new(target)
                        .with_parent(ProofV2TargetParent::new(parent_len))])
                    .unwrap();
                assert_eq!(proof.len(), 1);
                assert_eq!(proof[0].path, expected.path);
                assert_eq!(alloy_rlp::encode(&proof[0].node), alloy_rlp::encode(&expected.node));
            }
            assert!(calculator.proof(&mut [ProofV2Target::new(key)]).is_err());
        }
    }

    #[test]
    fn partial_absence_proof_ignores_neighbors_outside_requested_child() {
        let key = B256::repeat_byte(0x11);
        let path = Nibbles::unpack(key);
        let leaf = StateTrieNode::Leaf { short_key_len: 58, value: U256::from(1) };
        let mut calculator = ProofCalculator::new(Cursor([(path, leaf)].into()));
        let mut absent = key;
        absent.0[2] = 0x22;
        let proof = calculator
            .proof(&mut [ProofV2Target::new(absent).with_parent(ProofV2TargetParent::new(4))])
            .unwrap();
        assert!(proof.is_empty());
    }

    #[test]
    fn partial_proof_rebases_superseded_database_leaf() {
        let path = Nibbles::unpack(B256::repeat_byte(0x11));
        let leaf = StateTrieNode::Leaf { short_key_len: 60, value: U256::from(1) };
        let mut calculator = ProofCalculator::new(Cursor([(path, leaf)].into()));
        let mut target = B256::repeat_byte(0x11);
        target.0[2] = 0x12;
        let expected =
            StateTrieNode::Leaf { short_key_len: 59, value: U256::from(1) }.proof_node(path);
        for key in [target, B256::repeat_byte(0x11)] {
            let proof = calculator
                .proof(&mut [ProofV2Target::new(key).with_parent(ProofV2TargetParent::new(4))])
                .unwrap();
            assert_eq!(proof.len(), 1);
            assert_eq!(proof[0].path, expected.path);
            assert_eq!(alloy_rlp::encode(&proof[0].node), alloy_rlp::encode(&expected.node));
        }
    }

    #[test]
    fn agrees_with_v2_for_present_absent_and_partial_targets() {
        for prefix_len in [0, 2, 31] {
            let storage: BTreeMap<_, _> = (1u64..100)
                .map(|i| {
                    let mut key = keccak256(i.to_be_bytes());
                    key.0[..prefix_len].fill(0);
                    (key, U256::from(i))
                })
                .collect();
            let mut legacy = crate::proof_v2::ProofCalculator::new(
                crate::trie_cursor::noop::NoopAccountTrieCursor::default(),
                crate::hashed_cursor::mock::MockHashedCursor::new(
                    std::sync::Arc::new(storage.clone()),
                    Default::default(),
                ),
            );
            let mut builder = StateTrieBuilder::default();
            let mut nodes = BTreeMap::new();
            let mut write = |p, n| {
                nodes.insert(p, n);
                Ok::<_, core::convert::Infallible>(())
            };
            for (k, v) in &storage {
                builder.push(*k, *v, &mut write).unwrap();
            }
            builder.finish(&mut write).unwrap();
            let mut calc = ProofCalculator::new(Cursor(nodes));
            let canonical = |nodes: Vec<ProofTrieNodeV2>| {
                nodes
                    .into_iter()
                    .map(|n| (n.path, alloy_rlp::encode(n.node)))
                    .collect::<BTreeMap<_, _>>()
            };
            let mut mixed_targets: Vec<_> = storage
                .keys()
                .rev()
                .flat_map(|key| {
                    let target = ProofV2Target::new(*key);
                    let StateTrieNode::Leaf { short_key_len, .. } =
                        calc.cursor.0[&target.key_nibbles]
                    else {
                        unreachable!()
                    };
                    [
                        target,
                        target.with_parent(ProofV2TargetParent::new(63 - short_key_len as usize)),
                    ]
                })
                .collect();
            let actual = calc.proof(&mut mixed_targets).unwrap();
            let expected = legacy
                .proof(&mut crate::proof_v2::StorageValueEncoder, &mut mixed_targets)
                .unwrap();
            assert_eq!(canonical(actual), canonical(expected));
            for key in storage.keys().copied().chain([B256::ZERO, B256::repeat_byte(0xff)]).chain(
                (200u64..456).map(|i| {
                    let mut key = keccak256(i.to_be_bytes());
                    key.0[..prefix_len].fill(0);
                    if prefix_len == 31 {
                        key.0[31] = i as u8;
                    }
                    key
                }),
            ) {
                for parent in [
                    ProofV2TargetParent::NONE,
                    ProofV2TargetParent::new(0),
                    ProofV2TargetParent::new(2),
                    ProofV2TargetParent::new(62),
                ] {
                    let target = ProofV2Target::new(key).with_parent(parent);
                    // Known parents must actually exist; the caller only supplies revealed
                    // branches.
                    if let Some(path) = parent.path(target.key_nibbles) &&
                        !matches!(calc.cursor.0.get(&path), Some(StateTrieNode::Branch { .. }))
                    {
                        continue
                    }
                    let actual = calc.proof(&mut [target]).unwrap();
                    let expected = legacy
                        .proof(&mut crate::proof_v2::StorageValueEncoder, &mut [target])
                        .unwrap();
                    assert_eq!(
                        canonical(actual),
                        canonical(expected),
                        "target={target:?} prefix={prefix_len}"
                    );
                }
            }
        }
    }
}
