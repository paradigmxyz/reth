//! Proof calculation from current leaves and cached branch hashes.
//!
//! The reader resolves each target range into ordered inputs. The builder constructs nodes from
//! those inputs and retains the requested paths. Leaf values can be encoded after traversal starts
//! other work, such as computing account storage roots.

use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::{TrieCursor, TrieStorageCursor},
};
use alloy_primitives::{B256, U256};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    prefix_set::PrefixSet, Nibbles, ProofTrieNodeV2, ProofV2Target, TrieNodeV2,
};
use tracing::instrument;

mod builder;
use builder::ProofBuilder;

mod reader;
use reader::{CachedBranch, ProofCursorState, ProofReader};

mod value;
pub use value::*;

mod node;

mod target;
pub(crate) use target::*;

#[cfg(test)]
mod overlay_tests;

static TRACE_TARGET: &str = "trie::proof_v2";

/// Generates proofs from current leaves and cached branch data.
///
/// Target groups are read independently, and returned nodes are sorted with children before their
/// parents. Construction buffers are reused across calls.
#[derive(Debug)]
pub struct ProofCalculator<TC, HC, VE: LeafValueEncoder> {
    trie_cursor: TC,
    hashed_cursor: HC,
    prefix_set: PrefixSet,
    reader_frames: Vec<CachedBranch>,
    builder: ProofBuilder<VE::DeferredEncoder>,
}

impl<TC, HC, VE: LeafValueEncoder> ProofCalculator<TC, HC, VE> {
    /// Creates a proof calculator with the supplied cursors.
    pub fn new(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self {
            trie_cursor,
            hashed_cursor,
            prefix_set: PrefixSet::default(),
            reader_frames: Vec::with_capacity(64),
            builder: ProofBuilder::new(),
        }
    }

    /// Sets the changed keys whose cached hashes must be recalculated.
    ///
    /// The reader also opens unchanged children when deletions can collapse their parent branch.
    pub fn with_prefix_set(mut self, prefix_set: PrefixSet) -> Self {
        self.prefix_set = prefix_set;
        self
    }
}

impl<TC, HC, VE> ProofCalculator<TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: LeafValueEncoder<Value = HC::Value>,
{
    fn proof_subtrie(
        &mut self,
        value_encoder: &mut VE,
        targets: SubTrieTargets<'_>,
        cursor_state: &mut ProofCursorState<VE::DeferredEncoder>,
    ) -> Result<(), StateProofError> {
        let result = {
            let mut reader = ProofReader::new(
                &mut self.trie_cursor,
                &mut self.hashed_cursor,
                value_encoder,
                &mut self.prefix_set,
                &targets,
                &mut self.reader_frames,
                cursor_state,
            );
            self.builder.build(&targets, || reader.next())
        };
        self.reader_frames.clear();
        if result.is_err() {
            self.builder.clear();
        }
        result
    }

    /// Generates nodes along the target paths, sorted with children before parents.
    ///
    /// Targets are sorted in place by known parent and then by key.
    #[instrument(target = TRACE_TARGET, level = "trace", skip_all)]
    pub fn proof(
        &mut self,
        value_encoder: &mut VE,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        let mut cursor_state = ProofCursorState::default();
        for group in iter_sub_trie_targets(targets) {
            self.proof_subtrie(value_encoder, group, &mut cursor_state)?;
        }
        Ok(self.builder.take_proofs())
    }

    /// Computes the root hash, or returns None for a proof that starts below the root.
    pub fn compute_root_hash(
        &mut self,
        proof_nodes: &[ProofTrieNodeV2],
    ) -> Result<Option<B256>, StateProofError> {
        Ok(self.builder.root_hash(proof_nodes))
    }

    /// Calculates the root node using the current leaf data.
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, value_encoder))]
    pub fn root_node(
        &mut self,
        value_encoder: &mut VE,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
        self.proof_subtrie(
            value_encoder,
            SubTrieTargets {
                lower_bound: Nibbles::new(),
                upper_bound: None,
                parent_prefix: None,
                targets: &[],
            },
            &mut ProofCursorState::default(),
        )?;
        let mut proofs = self.builder.take_proofs();
        debug_assert_eq!(proofs.len(), 1);
        Ok(proofs.pop().expect("root calculation retains one node"))
    }
}

/// A proof calculator for storage tries.
pub type StorageProofCalculator<TC, HC> = ProofCalculator<TC, HC, StorageValueEncoder>;

impl<TC, HC> StorageProofCalculator<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
{
    /// Create a new [`StorageProofCalculator`] instance.
    pub fn new_storage(trie_cursor: TC, hashed_cursor: HC) -> Self {
        Self::new(trie_cursor, hashed_cursor)
    }

    /// Generate a proof for a storage trie at the given hashed address.
    ///
    /// Given a set of [`ProofV2Target`]s, returns nodes whose paths are a prefix of any target. The
    /// returned nodes will be sorted depth-first by path.
    ///
    /// Targets are sorted in place by known parent and then by key.
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self, targets))]
    pub fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        // Shortcut: check if storage is empty
        if self.hashed_cursor.is_storage_empty()? {
            return Ok(if targets.iter().any(|target| !target.parent.is_known()) {
                vec![ProofTrieNodeV2 {
                    path: Nibbles::default(),
                    node: TrieNodeV2::EmptyRoot,
                    masks: None,
                }]
            } else {
                Vec::new()
            })
        }

        // Don't call `set_hashed_address` on the trie cursor until after the previous shortcut has
        // been checked.
        self.trie_cursor.set_hashed_address(hashed_address);

        // Create a mutable storage value encoder
        let mut storage_value_encoder = StorageValueEncoder;
        self.proof(&mut storage_value_encoder, targets)
    }

    /// Calculates the root node of a storage trie.
    ///
    /// This method does not accept targets nor retain proofs. Returns the root node which can
    /// be used to compute the root hash via [`Self::compute_root_hash`].
    #[instrument(target = TRACE_TARGET, level = "trace", skip(self))]
    pub fn storage_root_node(
        &mut self,
        hashed_address: B256,
    ) -> Result<ProofTrieNodeV2, StateProofError> {
        self.hashed_cursor.set_hashed_address(hashed_address);

        if self.hashed_cursor.is_storage_empty()? {
            return Ok(ProofTrieNodeV2 {
                path: Nibbles::default(),
                node: TrieNodeV2::EmptyRoot,
                masks: None,
            })
        }

        // Don't call `set_hashed_address` on the trie cursor until after the previous shortcut has
        // been checked.
        self.trie_cursor.set_hashed_address(hashed_address);

        // Create a mutable storage value encoder
        let mut storage_value_encoder = StorageValueEncoder;
        self.root_node(&mut storage_value_encoder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        proof::StorageProof as LegacyStorageProof,
        test_utils::TrieTestHarness,
        trie_cursor::{depth_first, TrieCursorFactory},
    };
    use alloy_primitives::{keccak256, map::B256Set};
    use alloy_rlp::{Decodable, Encodable};
    use alloy_trie::{proof::AddedRemovedKeys, BranchNodeCompact, TrieMask};
    use itertools::Itertools;
    use reth_trie_common::{
        prefix_set::PrefixSetMut, ProofTrieNode, ProofV2TargetParent, TrieNode, EMPTY_ROOT_HASH,
    };
    use std::collections::BTreeMap;

    /// Converts legacy proofs to V2 proofs by combining extension nodes with their child branch
    /// nodes.
    ///
    /// In the legacy proof format, extension nodes and branch nodes are separate. In the V2 format,
    /// they are combined into a single `BranchNodeV2` where the extension's key becomes the
    /// branch's `key` field.
    ///
    /// Converts legacy proofs (sorted in depth-first order) to V2 format.
    ///
    /// In depth-first order, children come BEFORE parents. So when we encounter an extension node,
    /// its child branch has already been processed and is in the result. We need to pop it and
    /// combine it with the extension.
    fn convert_legacy_proofs_to_v2(legacy_proofs: &[ProofTrieNode]) -> Vec<ProofTrieNodeV2> {
        ProofTrieNodeV2::from_sorted_trie_nodes(
            legacy_proofs.iter().map(|p| (p.path, p.node.clone(), p.masks)),
        )
    }

    /// Projects a legacy proof node into the representation requested by a V2 target.
    fn project_legacy_proof_node(
        node: &ProofTrieNodeV2,
        target: &ProofV2Target,
    ) -> Option<ProofTrieNodeV2> {
        let Some(parent_path_len) = target.parent.path_len() else {
            return target.key_nibbles.starts_with(&node.path).then(|| node.clone())
        };

        if node.path.len() > parent_path_len {
            return target.key_nibbles.starts_with(&node.path).then(|| node.clone())
        }

        let logical_path = match &node.node {
            TrieNodeV2::Leaf(leaf) => node.path.join(&leaf.key),
            TrieNodeV2::Branch(branch) => node.path.join(&branch.key),
            TrieNodeV2::EmptyRoot | TrieNodeV2::Extension(_) => return None,
        };
        let child_path_len = parent_path_len + 1;
        if logical_path.len() < child_path_len {
            return None
        }

        let child_path = logical_path.slice(0..child_path_len);
        if !target.key_nibbles.starts_with(&child_path) {
            return None
        }

        let trim_len = child_path_len - node.path.len();
        let mut projected = node.clone();
        projected.path = child_path;
        match &mut projected.node {
            TrieNodeV2::Leaf(leaf) => leaf.key = leaf.key.slice(trim_len..),
            TrieNodeV2::Branch(branch) => {
                branch.key = branch.key.slice(trim_len..);
                if branch.key.is_empty() {
                    branch.branch_rlp_node = None;
                }
            }
            TrieNodeV2::EmptyRoot | TrieNodeV2::Extension(_) => unreachable!(),
        }
        Some(projected)
    }

    /// Builds the exact V2 representation expected for a legacy proof and set of targets.
    fn project_legacy_proof(
        legacy_nodes: &[ProofTrieNodeV2],
        targets: &[ProofV2Target],
    ) -> Vec<ProofTrieNodeV2> {
        let mut projected = targets
            .iter()
            .flat_map(|target| {
                legacy_nodes.iter().filter_map(move |node| project_legacy_proof_node(node, target))
            })
            .collect::<Vec<_>>();
        projected.sort_unstable_by(|a, b| depth_first::cmp(&a.path, &b.path));
        projected.dedup_by(|a, b| {
            if a.path != b.path {
                return false
            }
            assert_eq!(a, b, "target projections disagree at path {:?}", a.path);
            true
        });
        projected
    }

    /// A test harness for comparing `StorageProofCalculator` and legacy `StorageProof`
    /// implementations.
    ///
    /// Wraps [`TrieTestHarness`] and adds a method to test that both proof implementations
    /// produce equivalent results for storage proofs.
    struct ProofTestHarness {
        inner: TrieTestHarness,
    }

    impl std::ops::Deref for ProofTestHarness {
        type Target = TrieTestHarness;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl ProofTestHarness {
        /// Creates a new test harness from a map of hashed storage slots to values.
        fn new(storage: BTreeMap<B256, U256>) -> Self {
            Self { inner: TrieTestHarness::new(storage) }
        }

        fn root_with_prefix_set(&self, prefix_set: PrefixSet) -> Option<B256> {
            let trie_cursor =
                self.trie_cursor_factory().storage_trie_cursor(self.hashed_address()).unwrap();
            let hashed_cursor =
                self.hashed_cursor_factory().hashed_storage_cursor(self.hashed_address()).unwrap();
            let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
                .with_prefix_set(prefix_set);
            let mut targets = [ProofV2Target::new(B256::ZERO)];
            let proof = calculator.storage_proof(self.hashed_address(), &mut targets).unwrap();
            calculator.compute_root_hash(&proof).unwrap()
        }

        /// Asserts that `StorageProofCalculator` and legacy `StorageProof` produce equivalent
        /// results for storage proofs.
        fn assert_proof(
            &self,
            targets: impl IntoIterator<Item = ProofV2Target>,
        ) -> Result<(), StateProofError> {
            let mut targets_vec = targets.into_iter().collect::<Vec<_>>();

            // Get v2 proof and root hash via harness
            let (proof_v2_result, root_hash) = self.proof_v2(&mut targets_vec);

            // Verify the root hash matches the expected root (if the proof contains a root
            // node)
            if let Some(root_hash) = root_hash {
                pretty_assertions::assert_eq!(self.original_root(), root_hash);
            }

            // Fully materialize the legacy proof so compressed branches can be projected across
            // arbitrary parent boundaries, including absence targets that diverge inside them.
            let legacy_targets = targets_vec
                .iter()
                .map(|target| B256::from_slice(&target.key_nibbles.pack()))
                .chain(self.storage().keys().copied())
                .collect::<B256Set>();

            // Call legacy StorageProof::storage_multiproof
            let proof_legacy_result = LegacyStorageProof::new_hashed(
                self.trie_cursor_factory(),
                self.hashed_cursor_factory(),
                self.hashed_address(),
            )
            .with_branch_node_masks(true)
            .with_added_removed_keys(Some(AddedRemovedKeys::default().with_assume_added(true)))
            .storage_multiproof(legacy_targets)?;

            // Decode and sort legacy proof nodes
            let proof_legacy_nodes = proof_legacy_result
                .subtree
                .iter()
                .map(|(path, node_enc)| {
                    let mut buf = node_enc.as_ref();
                    let node = TrieNode::decode(&mut buf)
                        .expect("legacy implementation should not produce malformed proof nodes");

                    let masks = if path.is_empty() {
                        None
                    } else {
                        proof_legacy_result.branch_node_masks.get(path).copied()
                    };

                    ProofTrieNode { path: *path, node, masks }
                })
                .sorted_by(|a, b| depth_first::cmp(&a.path, &b.path))
                .collect::<Vec<_>>();

            // Convert legacy proofs to V2 proofs by combining extensions with their child branches
            let all_legacy_nodes_v2 = convert_legacy_proofs_to_v2(&proof_legacy_nodes);

            let expected_v2 = project_legacy_proof(&all_legacy_nodes_v2, &targets_vec);
            pretty_assertions::assert_eq!(expected_v2, proof_v2_result);

            Ok(())
        }
    }

    #[test]
    fn test_proof_through_64_branch_levels() {
        let storage = (0..=64)
            .map(|len| {
                let mut key = B256::ZERO;
                for nibble in 0..len {
                    key[nibble / 2] |= if nibble % 2 == 0 { 0xf0 } else { 0x0f };
                }
                (key, U256::from(len + 1))
            })
            .collect::<BTreeMap<_, _>>();
        let expected_root = crate::test_utils::storage_root_prehashed(storage.clone());
        let mut targets = storage.keys().copied().map(ProofV2Target::new).collect::<Vec<_>>();
        let hashed_factory = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((B256::ZERO, storage)).collect(),
        );
        let mut calculator = StorageProofCalculator::new_storage(
            crate::trie_cursor::noop::NoopStorageTrieCursor::default(),
            hashed_factory.hashed_storage_cursor(B256::ZERO).unwrap(),
        );
        let full = calculator.storage_proof(B256::ZERO, &mut targets).unwrap();
        assert_eq!(calculator.compute_root_hash(&full).unwrap(), Some(expected_root));

        let mut partial = [0, 1, 15, 31, 62, 63].map(|depth| {
            ProofV2Target::new(B256::repeat_byte(0xff)).with_parent(ProofV2TargetParent::new(depth))
        });
        let expected = project_legacy_proof(&full, &partial);
        let actual = calculator.storage_proof(B256::ZERO, &mut partial).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_proof_calculator_reuse_after_error() {
        use std::{cell::Cell, rc::Rc};

        struct Encoder(Rc<Cell<usize>>);
        struct Deferred(U256, Rc<Cell<usize>>);
        impl LeafValueEncoder for Encoder {
            type Value = U256;
            type DeferredEncoder = Deferred;
            fn deferred_encoder(&mut self, _: B256, value: U256) -> Deferred {
                Deferred(value, self.0.clone())
            }
        }
        impl DeferredValueEncoder for Deferred {
            fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
                let remaining = self.1.get();
                self.1.set(remaining.saturating_sub(1));
                if remaining == 1 {
                    return Err(StateProofError::TrieInconsistency(
                        "injected encoding error".into(),
                    ));
                }
                self.0.encode(buf);
                Ok(())
            }
        }

        let slots = [0x10, 0x20, 0x30, 0x40].map(|byte| B256::right_padding_from(&[byte]));
        let harness =
            ProofTestHarness::new(slots.into_iter().map(|key| (key, U256::from(100))).collect());
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
        let mut encoder = Encoder(Rc::new(Cell::new(2)));
        let mut targets = slots.map(ProofV2Target::new);
        assert!(calculator.proof(&mut encoder, &mut targets).is_err());
        let actual = calculator.proof(&mut encoder, &mut targets).unwrap();
        let (expected, _) = harness.proof_v2(&mut targets);
        pretty_assertions::assert_eq!(expected, actual);
    }

    #[test]
    fn test_partial_storage_proof_after_root_calculation() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd4, 0x00]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4, 0x10]);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let hashed_address = harness.hashed_address();
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(hashed_address).unwrap();
        let hashed_cursor =
            harness.hashed_cursor_factory().hashed_storage_cursor(hashed_address).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        let root_node = calculator.storage_root_node(hashed_address).unwrap();
        assert_eq!(
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap(),
            Some(harness.original_root())
        );

        let target = ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3));
        let mut actual_targets = [target];
        let actual = calculator.storage_proof(hashed_address, &mut actual_targets).unwrap();
        let mut expected_targets = [target];
        let (expected, root) = harness.proof_v2(&mut expected_targets);

        assert!(root.is_none());
        pretty_assertions::assert_eq!(expected, actual);
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a strategy for storage datasets (hashed slot → value).
        fn storage_strategy() -> impl Strategy<Value = BTreeMap<B256, U256>> {
            prop::collection::vec((any::<[u8; 32]>(), any::<u64>()), 0..=100).prop_map(|slots| {
                slots
                    .into_iter()
                    .map(|(slot_bytes, value)| (B256::from(slot_bytes), U256::from(value)))
                    .filter(|(_, v)| *v != U256::ZERO)
                    .collect()
            })
        }

        /// Generate a strategy for proof targets that are 80% from existing storage slots
        /// and 20% random keys. Each target has a random parent path length of `None` or 0..15.
        fn proof_targets_strategy(
            slot_keys: Vec<B256>,
        ) -> impl Strategy<Value = Vec<ProofV2Target>> {
            let num_slots = slot_keys.len();

            let target_count = 0..=(num_slots + 5);

            target_count.prop_flat_map(move |count| {
                let slot_keys = slot_keys.clone();
                prop::collection::vec(
                    (
                        prop::bool::weighted(0.8).prop_flat_map(move |from_slots| {
                            if from_slots && !slot_keys.is_empty() {
                                prop::sample::select(slot_keys.clone()).boxed()
                            } else {
                                any::<[u8; 32]>().prop_map(B256::from).boxed()
                            }
                        }),
                        0u8..16u8,
                    )
                        .prop_map(|(key, encoded_parent_path_len)| {
                            let parent = encoded_parent_path_len.checked_sub(1).map_or(
                                ProofV2TargetParent::NONE,
                                |parent_path_len| {
                                    ProofV2TargetParent::new(usize::from(parent_path_len))
                                },
                            );
                            ProofV2Target::new(key).with_parent(parent)
                        }),
                    count,
                )
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(4000))]
            #[test]
            /// Tests that `StorageProofCalculator` produces valid proofs for randomly generated
            /// storage datasets with proof targets.
            fn proptest_proof_with_targets(
                (storage, targets) in storage_strategy()
                    .prop_flat_map(|storage| {
                        let mut slot_keys: Vec<B256> = storage.keys().copied().collect();
                        slot_keys.sort_unstable();
                        let targets_strategy = proof_targets_strategy(slot_keys);
                        (Just(storage), targets_strategy)
                    })
            ) {
                reth_tracing::init_test_tracing();
                let harness = ProofTestHarness::new(storage);

                harness.assert_proof(targets).expect("Proof generation failed");
            }
        }
    }

    #[test]
    fn test_exact_subtrie_targets_with_root_target() {
        reth_tracing::init_test_tracing();

        let slot_80 = B256::right_padding_from(&[0x80]);
        let slot_82 = B256::right_padding_from(&[0x82]);
        let slot_f0 = B256::right_padding_from(&[0xf0]);
        let storage = BTreeMap::from([
            (slot_80, U256::from(1)),
            (slot_82, U256::from(2)),
            (slot_f0, U256::from(3)),
        ]);
        let targets = [
            ProofV2Target::new(B256::ZERO),
            ProofV2Target::new(slot_80).with_parent(ProofV2TargetParent::new(1)),
        ];

        let harness = ProofTestHarness::new(storage);
        harness.assert_proof(targets).expect("Proof generation failed");
    }

    #[test]
    fn test_rebases_singleton_subtrie_root_below_known_parent() {
        let slot = B256::right_padding_from(&[0xae, 0xd4, 0x09]);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, slot_nibbles.slice(0..4));
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("singleton subtrie root should remain a leaf")
        };
        assert_eq!(leaf.key, slot_nibbles.slice(4..));
    }

    #[test]
    fn test_rebases_singleton_leaf_at_max_parent_depth() {
        let slot = B256::repeat_byte(0xae);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(63))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, slot_nibbles);
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("singleton subtrie root should remain a leaf")
        };
        assert!(leaf.key.is_empty());
    }

    #[test]
    fn test_root_and_root_parent_targets_retain_both_singleton_representations() {
        let slot = B256::right_padding_from(&[0x20]);
        let slot_nibbles = Nibbles::unpack(slot);
        let harness = ProofTestHarness::new(BTreeMap::from([(slot, U256::from(1))]));
        let mut targets = [
            ProofV2Target::new(slot),
            ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(0)),
        ];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert_eq!(root, Some(harness.original_root()));
        let root_node = proof.iter().find(|node| node.path.is_empty()).expect("root proof");
        let TrieNodeV2::Leaf(root_leaf) = &root_node.node else { panic!("root should be a leaf") };
        assert_eq!(root_leaf.key, slot_nibbles);

        let child_path = slot_nibbles.slice(0..1);
        let child_node =
            proof.iter().find(|node| node.path == child_path).expect("rebased root child proof");
        let TrieNodeV2::Leaf(child_leaf) = &child_node.node else {
            panic!("root child should be a leaf")
        };
        assert_eq!(child_leaf.key, slot_nibbles.slice(1..));
    }

    #[test]
    fn test_rebases_compressed_branch_subtrie_root() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd4, 0x00]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4, 0x10]);
        let slot_nibbles = Nibbles::unpack(slot_a);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let mut targets = [ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        let branch_path = slot_nibbles.slice(0..4);
        let branch_node =
            proof.iter().find(|node| node.path == branch_path).expect("rebased compressed branch");
        let TrieNodeV2::Branch(branch) = &branch_node.node else {
            panic!("rebased node should be a branch")
        };
        assert!(branch.key.is_empty());
        assert!(branch.branch_rlp_node.is_none());
    }

    #[test]
    fn test_discards_reconstructed_known_parent_branch() {
        let slot_a = B256::right_padding_from(&[0xae, 0xd2]);
        let slot_b = B256::right_padding_from(&[0xae, 0xd4]);
        let slot_nibbles = Nibbles::unpack(slot_a);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (slot_a, U256::from(1)),
            (slot_b, U256::from(2)),
        ]));
        let mut targets = [ProofV2Target::new(slot_a).with_parent(ProofV2TargetParent::new(3))];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert!(!proof.iter().any(|node| node.path == slot_nibbles.slice(0..3)));
        assert!(proof.iter().any(|node| node.path == slot_nibbles.slice(0..4)));
    }

    #[test]
    fn test_rebased_root_matches_direct_child_not_full_short_key() {
        let stored_slot = B256::right_padding_from(&[0xae, 0xd4, 0x09]);
        let same_child_target = B256::right_padding_from(&[0xae, 0xd4, 0xff]);
        let other_child_target = B256::right_padding_from(&[0xae, 0xd5]);
        let harness = ProofTestHarness::new(BTreeMap::from([(stored_slot, U256::from(1))]));

        let mut same_child =
            [ProofV2Target::new(same_child_target).with_parent(ProofV2TargetParent::new(3))];
        let (proof, _) = harness.proof_v2(&mut same_child);
        assert_eq!(proof.len(), 1, "divergent leaf proves absence below the same child");

        let mut other_child =
            [ProofV2Target::new(other_child_target).with_parent(ProofV2TargetParent::new(3))];
        let (proof, _) = harness.proof_v2(&mut other_child);
        assert!(proof.is_empty(), "a different direct child is unrelated to the target");
    }

    #[test]
    fn test_known_parent_sibling_span_retains_only_target_children() {
        let stored_slot_a = B256::right_padding_from(&[0xea, 0x53]);
        let stored_slot_b = B256::right_padding_from(&[0xeb, 0x53]);
        let stored_slot_c = B256::right_padding_from(&[0xec, 0x53]);
        let target_a = B256::right_padding_from(&[0xea, 0x1f]);
        let target_c = B256::right_padding_from(&[0xec, 0x1f]);
        let harness = ProofTestHarness::new(BTreeMap::from([
            (stored_slot_a, U256::from(1)),
            (stored_slot_b, U256::from(2)),
            (stored_slot_c, U256::from(3)),
        ]));
        let mut targets = [target_a, target_c]
            .map(|target| ProofV2Target::new(target).with_parent(ProofV2TargetParent::new(1)));

        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(
            proof.iter().map(|node| node.path).collect::<Vec<_>>(),
            [Nibbles::from_nibbles([0xe, 0xa]), Nibbles::from_nibbles([0xe, 0xc])]
        );
    }

    #[test]
    fn test_known_parent_does_not_use_stale_parent_mask() {
        let stored_slot_a = B256::right_padding_from(&[0xea, 0x53]);
        let stored_slot = B256::right_padding_from(&[0xeb, 0x53]);
        let stored_slot_c = B256::right_padding_from(&[0xec, 0x53]);
        let target = B256::right_padding_from(&[0xeb, 0x1f]);
        let stored_slot_nibbles = Nibbles::unpack(stored_slot);

        // The known parent at `e` is supplied by the sparse trie and may be stale in the database
        // when partial persistence masks that path. In particular, its state mask can omit the
        // live `eb` child while hashed state already contains that child's leaf.
        let stale_parent_mask = TrieMask::new((1 << 0xa) | (1 << 0xc));
        let stale_parent = BranchNodeCompact::new(
            stale_parent_mask,
            TrieMask::new(0),
            TrieMask::new(0),
            Vec::new(),
            None,
        );
        let storage_nodes = BTreeMap::from([(Nibbles::from_nibbles([0xe]), stale_parent)]);

        let mut harness = TrieTestHarness::new(BTreeMap::from([
            (stored_slot_a, U256::from(1)),
            (stored_slot, U256::from(2)),
            (stored_slot_c, U256::from(3)),
        ]));
        harness.set_trie_nodes(storage_nodes);

        let mut targets = [ProofV2Target::new(target).with_parent(ProofV2TargetParent::new(1))];
        let (proof, root) = harness.proof_v2(&mut targets);

        assert!(root.is_none());
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].path, stored_slot_nibbles.slice(0..2));
        let TrieNodeV2::Leaf(leaf) = &proof[0].node else {
            panic!("live direct child should be reconstructed as a leaf")
        };
        assert_eq!(leaf.key, stored_slot_nibbles.slice(2..));
    }

    #[test]
    fn test_empty_storage_respects_parent_context() {
        let harness = ProofTestHarness::new(BTreeMap::new());
        let slot = B256::ZERO;

        let mut partial_target =
            [ProofV2Target::new(slot).with_parent(ProofV2TargetParent::new(0))];
        let (partial_proof, partial_root) = harness.proof_v2(&mut partial_target);
        assert!(partial_proof.is_empty());
        assert!(partial_root.is_none());

        let mut root_target = [ProofV2Target::new(slot)];
        let (root_proof, root) = harness.proof_v2(&mut root_target);
        assert_eq!(root_proof.len(), 1);
        assert!(matches!(root_proof[0].node, TrieNodeV2::EmptyRoot));
        assert_eq!(root, Some(EMPTY_ROOT_HASH));
    }

    #[test]
    fn test_big_trie() {
        use rand::prelude::*;

        reth_tracing::init_test_tracing();
        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);

        let mut rand_b256 = || {
            let mut buf: [u8; 32] = [0; 32];
            rng.fill_bytes(&mut buf);
            B256::from_slice(&buf)
        };

        // Generate random storage dataset.
        let mut storage = BTreeMap::new();
        for _ in 0..10240 {
            let hashed_slot = rand_b256();
            storage.insert(hashed_slot, U256::from(1u64));
        }

        // Collect targets; partially from real keys, partially random keys which probably won't
        // exist.
        let mut targets = storage.keys().copied().collect::<Vec<_>>();
        for _ in 0..storage.len() / 5 {
            targets.push(rand_b256());
        }
        targets.sort();

        // Create test harness
        let harness = ProofTestHarness::new(storage);

        harness
            .assert_proof(targets.into_iter().map(ProofV2Target::new))
            .expect("Proof generation failed");
    }

    #[test]
    fn test_node_with_masked_empty_child() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);

        // All storage keys share a common first nibble (0x6), so the branch is at path 0x6. The
        // second nibble differentiates children: 0,1,3,5,7.
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_65 = B256::right_padding_from(&[0x65]);
        let slot_67 = B256::right_padding_from(&[0x67]);

        // Construct a branch node at path 0x6 with state_mask bits 0,1,3,5,7.
        // hash_mask has bits 0,1,5,7 (NOT 3) — nibble 3's hash is cleared because it's in the
        // prefix set. Hashes are dummy values.
        let state_mask = TrieMask::new(0b10101011); // bits 0,1,3,5,7
        let hash_mask = TrieMask::new(0b10100011); // bits 0,1,5,7 (NOT 3)
        let hashes = vec![B256::repeat_byte(0xaa); hash_mask.count_ones() as usize];
        let branch = BranchNodeCompact::new(state_mask, TrieMask::new(0), hash_mask, hashes, None);

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), branch)).collect();

        // Hashed cursor has slots at children 0, 1, 5, 7 — but NOT child 3 (0x63).
        // This simulates the post-state overlay having deleted the slot at 0x63.
        let mut harness = TrieTestHarness::new(
            [slot_60, slot_61, slot_65, slot_67].iter().map(|s| (*s, val)).collect(),
        );
        harness.set_trie_nodes(storage_nodes);

        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with masked empty child");

        let root_hash = calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap();
        assert!(root_hash.is_some(), "should produce a root hash");
    }

    /// Tests that `root_node` handles the case where `uncalculated_lower_bound` has advanced
    /// entirely past a cached branch that still has unprocessed children in its `state_mask`.
    ///
    /// Branch at `0x6` has `state_mask` bits 0,1,5,f where nibble 5 has its `hash_mask`
    /// cleared and no leaf data. The last child (nibble f)
    /// causes `calculate_key_range` to be called with range `(0x6f, Some(0x7))`. After that range,
    /// the hashed cursor still has keys (at `0x70...`), so `proof_subtrie` does not break and
    /// re-enters `next_uncached_key_range` with `uncalculated_lower_bound = Some(0x7)`.
    /// Since `0x7` is past `0x6`, all remaining children are skipped and the branch is popped.
    #[test]
    fn test_node_with_masked_empty_child_lower_bound_past_branch() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);

        // Leaf keys under 0x6 and one beyond (0x70) to keep the cursor alive after 0x6.
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_6f = B256::right_padding_from(&[0x6f]);
        let slot_70 = B256::right_padding_from(&[0x70]);

        // Branch at 0x6: state_mask bits 0,1,5,f; hash_mask bits 0,1 (NOT 5, NOT f).
        // Nibble 5 has state_mask set but no hash and no leaf data (masked empty child).
        // Nibble f has state_mask set, no hash, but DOES have leaf data.
        let state_mask = TrieMask::new(0b1000_0000_0010_0011); // bits 0,1,5,f
        let hash_mask = TrieMask::new(0b0000_0000_0000_0011); // bits 0,1
        let hashes = vec![B256::repeat_byte(0xaa); hash_mask.count_ones() as usize];
        let branch = BranchNodeCompact::new(state_mask, TrieMask::new(0), hash_mask, hashes, None);

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), branch)).collect();

        // Hashed cursor: slots at 0x60, 0x61, 0x6f, 0x70 — but NOT 0x65.
        let mut harness = TrieTestHarness::new(
            [slot_60, slot_61, slot_6f, slot_70].iter().map(|s| (*s, val)).collect(),
        );
        harness.set_trie_nodes(storage_nodes);

        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed when lower bound advances past branch");

        let root_hash = calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap();
        assert!(root_hash.is_some(), "should produce a root hash");
    }

    /// Tests that the prefix set causes `next_uncached_key_range` to add child nibbles that are
    /// not present in the cached branch's `state_mask`.
    ///
    /// Setup: An original state with leaves at `0x60` and `0x61` produces a cached branch at
    /// `0x6` with children at nibbles 0 and 1 (both with real cached hashes from `StorageRoot`).
    /// A new leaf is then inserted at `0x63...`, which is NOT in the branch's `state_mask`.
    /// The prefix set contains the new key. Without prefix set support, the calculator would
    /// skip nibble 3 entirely and produce a stale root hash. With prefix set support, nibble 3
    /// is discovered and its subtrie is recalculated from leaves.
    #[test]
    fn test_prefix_set_adds_child_nibbles() {
        reth_tracing::init_test_tracing();

        let val = U256::from(42u64);
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_63 = B256::right_padding_from(&[0x63]);

        let harness = TrieTestHarness::new([(slot_60, val), (slot_61, val)].into_iter().collect());

        let changeset: BTreeMap<B256, U256> = std::iter::once((slot_63, val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(slot_63, val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(slot_63));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with prefix set adding child nibbles");
        let got_root =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash with prefix set should match fresh computation"
        );
    }

    /// Tests that `next_uncached_key_range` does not use a cached hash when the child's path
    /// is in the prefix set, forcing recalculation from leaves.
    ///
    /// Setup: A cached branch at `0x6` with children at nibbles 0,1,5 — all with cached hashes.
    /// The leaf at `0x65...` is changed (different value). The prefix set marks `0x65...` as
    /// dirty. Without prefix set support, the calculator would use the stale cached hash for
    /// nibble 5 and produce a wrong root. With prefix set support, the cached hash is skipped
    /// and the subtrie is recalculated from the updated leaf data.
    #[test]
    fn test_prefix_set_invalidates_cached_hash() {
        reth_tracing::init_test_tracing();

        let original_val = U256::from(42u64);
        let updated_val = U256::from(9999u64);
        let slot_60 = B256::right_padding_from(&[0x60]);
        let slot_61 = B256::right_padding_from(&[0x61]);
        let slot_65 = B256::right_padding_from(&[0x65]);

        let harness = TrieTestHarness::new(
            [(slot_60, original_val), (slot_61, original_val), (slot_65, original_val)]
                .into_iter()
                .collect(),
        );

        let changeset: BTreeMap<B256, U256> = std::iter::once((slot_65, updated_val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(slot_65, updated_val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(slot_65));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed with prefix set invalidating cached hash");
        let got_root =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash with prefix set invalidation should match fresh computation"
        );
    }

    fn b256(s: &str) -> B256 {
        B256::from_slice(&alloy_primitives::hex::decode(s).expect("valid hex string"))
    }

    #[test]
    fn test_prefix_set_root_proof_processes_sibling_after_cached_descendant() {
        reth_tracing::init_test_tracing();

        let storage = [
            ("1022c69e9d900e40775cd387c134899f465f291dbc3c97899ff6bfb8dc972b37", 45u64),
            ("1111ad8083c8a3a398b2b781217b989ff4d1ed182f46cc765eda49a7b316139d", 60),
            ("12012d20943649899b2fc0f87b9840b70ef68e93613aac17c269bf8c5a78a712", 17),
            ("12014b57b9a162c03d072eb6acd4e936f1c4bc23b803a054347c5ee9a9bcfb9a", 49),
            ("1203f800840af3f898ab4572f2750106a7c4bd2b3e844b6e7fa72704673cc2c6", 76),
            ("12208f18fbcd6971c92808721392acbf11d5af58e9143a374cc86e70bdd1f097", 10),
        ]
        .into_iter()
        .map(|(key, value)| (b256(key), U256::from(value)))
        .collect();

        let dirty = b256("12208f18fbcd6971c92808721392acbf11d5af58e9143a374cc86e70bdd1f097");
        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
            "root proof must process a prefix-set sibling after a cached descendant",
        );
    }

    #[test]
    fn test_prefix_set_root_proof_processes_trailing_dirty_sibling() {
        reth_tracing::init_test_tracing();

        let keys = [
            "0022001020000000000000000000000000000000000000000000000000000000",
            "0110212112000000000000000000000000000000000000000000000000000000",
            "0202210210000000000000000000000000000000000000000000000000000000",
            "0211020211000000000000000000000000000000000000000000000000000000",
            "0211211002000000000000000000000000000000000000000000000000000000",
            "0212221010000000000000000000000000000000000000000000000000000000",
            "0222011102000000000000000000000000000000000000000000000000000000",
        ];
        let storage =
            keys.iter().enumerate().map(|(i, key)| (b256(key), U256::from(i as u64 + 1))).collect();
        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        // The dirty children straddle a clean cached descendant under branch 0x02. Traversal
        // must resume at the trailing dirty sibling after using the cached descendant.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(b256(keys[2])));
        prefix_set.insert(Nibbles::unpack(b256(keys[6])));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
        );
    }

    /// Helper to compute the keccak256 hash of a storage leaf node. The `short_key` is the
    /// leaf's key after trimming all branch/extension nibbles consumed by ancestor nodes.
    fn storage_leaf_hash(short_key: &Nibbles, value: &U256) -> B256 {
        let mut buf = Vec::new();
        alloy_trie::nodes::LeafNodeRef::new(short_key, &alloy_rlp::encode_fixed_size(value))
            .encode(&mut buf);
        keccak256(&buf)
    }

    /// Tests branch collapse when the removed child comes BEFORE the remaining child.
    ///
    /// Trie structure (3 hashed storage keys):
    ///   `key_a` = 0x20...  (root nibble 2, sub-nibble 0)
    ///   `key_b` = 0x21...  (root nibble 2, sub-nibble 1)
    ///   `key_c` = 0xb0...  (root nibble b)
    ///
    /// This creates:
    ///   root branch at nibbles {2, b}
    ///   sub-branch at path [2] at nibbles {0, 1}
    ///
    /// `key_a` is removed (prefix set marks it dirty, cursor has no value for it).
    /// The sub-branch at [2] collapses into its remaining child (`key_b`). The removed child
    /// (nibble 0) comes before the remaining child (nibble 1).
    #[test]
    fn test_branch_collapse_removed_child_before_remaining() {
        reth_tracing::init_test_tracing();

        let val = U256::from(1u64);

        let key_a = B256::right_padding_from(&[0x20]); // root nibble 2, sub-nibble 0
        let key_b = B256::right_padding_from(&[0x21]); // root nibble 2, sub-nibble 1
        let key_c = B256::right_padding_from(&[0xb0]); // root nibble b

        // Compute leaf hashes for the sub-branch's children.
        // The sub-branch at path [2] consumes 2 nibbles from each key (root nibble + sub-nibble).
        let leaf_hash_a = storage_leaf_hash(&Nibbles::unpack(key_a).slice(2..), &val);
        let leaf_hash_b = storage_leaf_hash(&Nibbles::unpack(key_b).slice(2..), &val);

        // Only cache the sub-branch at path [2] — the root will be built from leaves.
        // The sub-branch has children at nibbles 0 and 1, both with cached hashes.
        let sub_branch_state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let cached_sub_branch = BranchNodeCompact::new(
            sub_branch_state_mask,
            TrieMask::new(0),
            sub_branch_state_mask,
            vec![leaf_hash_a, leaf_hash_b],
            None,
        );

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x2]), cached_sub_branch)).collect();

        // The hashed cursor contains key_b and key_c (the root's other child). key_a was removed
        // (not in cursor)
        let mut harness = TrieTestHarness::new([(key_b, val), (key_c, val)].into_iter().collect());
        harness.set_trie_nodes(storage_nodes);

        // Prefix set marks key_a as dirty (removed).
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::unpack(key_a));
        let prefix_set = prefix_set_mut.freeze();

        // Compute root with cached branches + prefix set — triggers sub-branch collapse.
        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor)
                .with_prefix_set(prefix_set);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed after branch collapse");
        let root_with_collapse =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        // Compute reference root from scratch (no cached branches) using the full final state.
        let mut fresh_harness =
            TrieTestHarness::new([(key_b, val), (key_c, val)].into_iter().collect());
        fresh_harness.set_trie_nodes(BTreeMap::new());
        let storage_trie_cursor = fresh_harness
            .trie_cursor_factory()
            .storage_trie_cursor(fresh_harness.hashed_address())
            .unwrap();
        let hashed_storage_cursor = fresh_harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(fresh_harness.hashed_address())
            .unwrap();
        let mut fresh_calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let fresh_root_node = fresh_calculator
            .storage_root_node(fresh_harness.hashed_address())
            .expect("fresh storage_root_node should succeed");
        let expected_root = fresh_calculator
            .compute_root_hash(core::slice::from_ref(&fresh_root_node))
            .unwrap()
            .unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            root_with_collapse,
            "Root hash after collapsing branch (removed child before remaining) should match fresh computation"
        );
    }

    /// Tests branch collapse when the removed child comes AFTER the remaining child.
    ///
    /// Same trie structure as "before" test, but with nibbles 4 and 9 instead of 0 and 1 for
    /// the sub-branch, and nibble 9 is removed. The removed child (nibble 9) comes after the
    /// remaining child (nibble 4).
    #[test]
    fn test_branch_collapse_removed_child_after_remaining() {
        reth_tracing::init_test_tracing();

        let val = U256::from(1u64);

        // key_a at sub-nibble 4, key_b at sub-nibble 9 (under root nibble 2).
        let key_a = B256::right_padding_from(&[0x24]); // root nibble 2, sub-nibble 4
        let key_b = B256::right_padding_from(&[0x29]); // root nibble 2, sub-nibble 9
        let key_c = B256::right_padding_from(&[0xb0]); // root nibble b

        let leaf_hash_a = storage_leaf_hash(&Nibbles::unpack(key_a).slice(2..), &val);
        let leaf_hash_b = storage_leaf_hash(&Nibbles::unpack(key_b).slice(2..), &val);

        // Only cache the sub-branch at path [2] — the root will be built from leaves.
        let sub_branch_state_mask = TrieMask::new((1 << 4) | (1 << 9));
        let cached_sub_branch = BranchNodeCompact::new(
            sub_branch_state_mask,
            TrieMask::new(0),
            sub_branch_state_mask,
            vec![leaf_hash_a, leaf_hash_b],
            None,
        );

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x2]), cached_sub_branch)).collect();

        // The hashed cursor contains key_a and key_c. key_b was removed (not in cursor)
        let mut harness = TrieTestHarness::new([(key_a, val), (key_c, val)].into_iter().collect());
        harness.set_trie_nodes(storage_nodes);

        // Prefix set marks key_b as dirty (removed).
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::unpack(key_b));
        let prefix_set = prefix_set_mut.freeze();

        // Compute root with cached branches + prefix set — triggers sub-branch collapse.
        let storage_trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_storage_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor)
                .with_prefix_set(prefix_set);
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed after branch collapse");
        let root_with_collapse =
            calculator.compute_root_hash(core::slice::from_ref(&root_node)).unwrap().unwrap();

        // Compute reference root from scratch (no cached branches) using the full final state.
        let mut fresh_harness =
            TrieTestHarness::new([(key_a, val), (key_c, val)].into_iter().collect());
        fresh_harness.set_trie_nodes(BTreeMap::new());
        let storage_trie_cursor = fresh_harness
            .trie_cursor_factory()
            .storage_trie_cursor(fresh_harness.hashed_address())
            .unwrap();
        let hashed_storage_cursor = fresh_harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(fresh_harness.hashed_address())
            .unwrap();
        let mut fresh_calculator =
            StorageProofCalculator::new_storage(storage_trie_cursor, hashed_storage_cursor);
        let fresh_root_node = fresh_calculator
            .storage_root_node(fresh_harness.hashed_address())
            .expect("fresh storage_root_node should succeed");
        let expected_root = fresh_calculator
            .compute_root_hash(core::slice::from_ref(&fresh_root_node))
            .unwrap()
            .unwrap();

        pretty_assertions::assert_eq!(
            expected_root,
            root_with_collapse,
            "Root hash after collapsing branch (removed child after remaining) should match fresh computation"
        );
    }

    #[test]
    fn test_cached_branch_extension_skips_diverging_target() {
        reth_tracing::init_test_tracing();

        let val = U256::from(100u64);

        // Keys whose first bytes directly set the nibble paths we need.
        let key_a0 = B256::right_padding_from(&[0x6a, 0x30]); // nibbles: 6,a,3,0,...
        let key_a1 = B256::right_padding_from(&[0x6a, 0x31]); // nibbles: 6,a,3,1,...
        let key_c = B256::right_padding_from(&[0x6a, 0x80]); // nibbles: 6,a,8,0,...
        let key_d = B256::right_padding_from(&[0x6b, 0x00]); // nibbles: 6,b,0,0,...
        let key_e = B256::right_padding_from(&[0x6c, 0x00]); // nibbles: 6,c,0,0,...

        // Build a correct trie from all five leaves to get the expected root and real hashes.
        let all_storage: BTreeMap<B256, U256> =
            [(key_a0, val), (key_a1, val), (key_c, val), (key_d, val), (key_e, val)]
                .into_iter()
                .collect();
        let correct_harness = TrieTestHarness::new(all_storage.clone());
        let expected_root = correct_harness.original_root();

        // Compute leaf hashes for constructing manual cached branch nodes.
        let leaf_hash_a0 = storage_leaf_hash(&Nibbles::unpack(key_a0).slice(4..), &val);
        let leaf_hash_a1 = storage_leaf_hash(&Nibbles::unpack(key_a1).slice(4..), &val);
        let leaf_hash_d = storage_leaf_hash(&Nibbles::unpack(key_d).slice(2..), &val);
        let leaf_hash_e = storage_leaf_hash(&Nibbles::unpack(key_e).slice(2..), &val);

        // ── Construct cached branch at [6] ─────────────────────────────────────
        // state_mask: bits a, b, and c set.
        // hash_mask:  bits b and c — both have cached leaf hashes.  Bit a has no hash, so the
        //             calculator will seek the trie cursor to find a deeper cached branch.
        //
        // Having three children with two (b, c) NOT in the prefix set ensures
        // `should_skip_cached_branch` does NOT skip this branch (num_unmatched >= 2).
        let branch_6_state_mask = TrieMask::new((1 << 0xa) | (1 << 0xb) | (1 << 0xc));
        let branch_6_hash_mask = TrieMask::new((1 << 0xb) | (1 << 0xc));
        let branch_6 = BranchNodeCompact::new(
            branch_6_state_mask,
            TrieMask::new(0),
            branch_6_hash_mask,
            vec![leaf_hash_d, leaf_hash_e],
            None,
        );

        // ── Construct cached branch at [6,a,3] ────────────────────────────────
        // state_mask: bits 0 and 1 set (children key_a0 and key_a1).
        // hash_mask:  both bits set — both children have cached hashes.
        let branch_6a3_state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let branch_6a3 = BranchNodeCompact::new(
            branch_6a3_state_mask,
            TrieMask::new(0),
            branch_6a3_state_mask,
            vec![leaf_hash_a0, leaf_hash_a1],
            None,
        );

        // Intentionally omit the branch at [6,a] — this is the inconsistency.
        let inconsistent_nodes: BTreeMap<Nibbles, BranchNodeCompact> = [
            (Nibbles::from_nibbles([0x6]), branch_6),
            (Nibbles::from_nibbles([0x6, 0xa, 0x3]), branch_6a3),
        ]
        .into_iter()
        .collect();

        // Create harness with all five leaves but the inconsistent trie nodes.
        let mut harness = TrieTestHarness::new(all_storage);
        harness.set_trie_nodes(inconsistent_nodes);

        // Mark key_c as dirty — in the real scenario the leaf was touched by execution.
        // The prefix set contains only key_c's full path. `should_skip_cached_branch` will
        // NOT skip branch [6] because two of its three children (b, c) are not in the set
        // (num_unmatched = 2 > 1). It also will not skip branch [6,a,3] because
        // `contains([6,a,3])` is false (key_c's nibbles 6,a,8,... do not start with 6,a,3).
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_c));

        // ── Verify root hash ───────────────────────────────────────────────────
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");
        let got_root = calculator
            .compute_root_hash(core::slice::from_ref(&root_node))
            .unwrap()
            .expect("should produce a root hash");

        // With the bug, the calculator skips key_c and produces a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match correct trie; cached extension must not skip diverging leaves"
        );

        // ── Verify proof for key_c contains nodes on its path ──────────────────
        let mut targets = vec![ProofV2Target::new(key_c)];
        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");

        let key_c_nibbles = Nibbles::unpack(key_c);
        let has_matching_node = proofs.iter().any(|node| key_c_nibbles.starts_with(&node.path));
        assert!(
            has_matching_node,
            "Proof for key_c should contain at least one node on key_c's path, got: {proofs:?}"
        );
    }

    #[test]
    fn test_cached_branch_extension_skips_diverging_target_before() {
        reth_tracing::init_test_tracing();

        let val = U256::from(100u64);

        // Keys whose first bytes directly set the nibble paths we need.
        let key_a0 = B256::right_padding_from(&[0x6a, 0x80]); // nibbles: 6,a,8,0,...
        let key_a1 = B256::right_padding_from(&[0x6a, 0x81]); // nibbles: 6,a,8,1,...
        let key_c = B256::right_padding_from(&[0x6a, 0x30]); // nibbles: 6,a,3,0,... (BEFORE [6,a,8])
        let key_d = B256::right_padding_from(&[0x6b, 0x00]); // nibbles: 6,b,0,0,...
        let key_e = B256::right_padding_from(&[0x6c, 0x00]); // nibbles: 6,c,0,0,...

        // Build a correct trie from all five leaves to get the expected root and real hashes.
        let all_storage: BTreeMap<B256, U256> =
            [(key_a0, val), (key_a1, val), (key_c, val), (key_d, val), (key_e, val)]
                .into_iter()
                .collect();
        let correct_harness = TrieTestHarness::new(all_storage.clone());
        let expected_root = correct_harness.original_root();

        // Compute leaf hashes for constructing manual cached branch nodes.
        let leaf_hash_a0 = storage_leaf_hash(&Nibbles::unpack(key_a0).slice(4..), &val);
        let leaf_hash_a1 = storage_leaf_hash(&Nibbles::unpack(key_a1).slice(4..), &val);
        let leaf_hash_d = storage_leaf_hash(&Nibbles::unpack(key_d).slice(2..), &val);
        let leaf_hash_e = storage_leaf_hash(&Nibbles::unpack(key_e).slice(2..), &val);

        // ── Construct cached branch at [6] ─────────────────────────────────────
        // state_mask: bits a, b, and c set.
        // hash_mask:  bits b and c — both have cached leaf hashes.  Bit a has no hash, so the
        //             calculator will seek the trie cursor to find a deeper cached branch.
        //
        // Having three children with two (b, c) NOT in the prefix set ensures
        // `should_skip_cached_branch` does NOT skip this branch (num_unmatched >= 2).
        let branch_6_state_mask = TrieMask::new((1 << 0xa) | (1 << 0xb) | (1 << 0xc));
        let branch_6_hash_mask = TrieMask::new((1 << 0xb) | (1 << 0xc));
        let branch_6 = BranchNodeCompact::new(
            branch_6_state_mask,
            TrieMask::new(0),
            branch_6_hash_mask,
            vec![leaf_hash_d, leaf_hash_e],
            None,
        );

        // ── Construct cached branch at [6,a,8] ────────────────────────────────
        // state_mask: bits 0 and 1 set (children key_a0 and key_a1).
        // hash_mask:  both bits set — both children have cached hashes.
        let branch_6a8_state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let branch_6a8 = BranchNodeCompact::new(
            branch_6a8_state_mask,
            TrieMask::new(0),
            branch_6a8_state_mask,
            vec![leaf_hash_a0, leaf_hash_a1],
            None,
        );

        // Intentionally omit the branch at [6,a] — this is the inconsistency.
        let inconsistent_nodes: BTreeMap<Nibbles, BranchNodeCompact> = [
            (Nibbles::from_nibbles([0x6]), branch_6),
            (Nibbles::from_nibbles([0x6, 0xa, 0x8]), branch_6a8),
        ]
        .into_iter()
        .collect();

        // Create harness with all five leaves but the inconsistent trie nodes.
        let mut harness = TrieTestHarness::new(all_storage);
        harness.set_trie_nodes(inconsistent_nodes);

        // Mark key_c as dirty — it comes BEFORE the cached branch [6,a,8] in nibble order.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_c));

        // ── Verify root hash ───────────────────────────────────────────────────
        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");
        let got_root = calculator
            .compute_root_hash(core::slice::from_ref(&root_node))
            .unwrap()
            .expect("should produce a root hash");

        // With the bug, the calculator skips key_c and produces a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match correct trie; cached extension must not skip diverging leaves before cached branch"
        );

        // ── Verify proof for key_c contains nodes on its path ──────────────────
        let mut targets = vec![ProofV2Target::new(key_c)];
        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");

        let key_c_nibbles = Nibbles::unpack(key_c);
        let has_matching_node = proofs.iter().any(|node| key_c_nibbles.starts_with(&node.path));
        assert!(
            has_matching_node,
            "Proof for key_c should contain at least one node on key_c's path, got: {proofs:?}"
        );
    }

    #[test]
    fn test_skipped_parent_branch_with_unskipped_child() {
        reth_tracing::init_test_tracing();

        let val = U256::from(1u64);
        let updated_val = U256::from(2u64);

        // We need cached branches at [2], [2,f], and [3] in the trie DB.
        let key_2 = B256::right_padding_from(&[0x20]);
        let key_2f00 = B256::right_padding_from(&[0x2f, 0x00]);
        let key_2f01 = B256::right_padding_from(&[0x2f, 0x01]);
        let key_2f10 = B256::right_padding_from(&[0x2f, 0x10]);
        let key_2f11 = B256::right_padding_from(&[0x2f, 0x11]);
        let key_300 = B256::right_padding_from(&[0x30, 0x00]);
        let key_301 = B256::right_padding_from(&[0x30, 0x10]);
        let key_310 = B256::right_padding_from(&[0x31, 0x00]);
        let key_311 = B256::right_padding_from(&[0x31, 0x10]);
        let key_500 = B256::right_padding_from(&[0x50, 0x00]);
        let key_501 = B256::right_padding_from(&[0x50, 0x10]);
        let key_510 = B256::right_padding_from(&[0x51, 0x00]);
        let key_511 = B256::right_padding_from(&[0x51, 0x10]);

        let all_keys = [
            key_2, key_2f00, key_2f01, key_2f10, key_2f11, key_300, key_301, key_310, key_311,
            key_500, key_501, key_510, key_511,
        ];

        let original_storage: BTreeMap<B256, U256> = all_keys.iter().map(|k| (*k, val)).collect();
        let harness = TrieTestHarness::new(original_storage);

        // Verify that the expected branches exist in the trie.
        let trie_updates = harness.storage_trie_updates();
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x2])));
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x2, 0xf])));
        assert!(trie_updates.storage_nodes.contains_key(&Nibbles::from_nibbles([0x3])));

        // Change only key_2 — triggers skip of parent branch [2] while child [2,f] is not
        // skipped.
        let changeset: BTreeMap<B256, U256> = std::iter::once((key_2, updated_val)).collect();
        let (expected_root, _) = harness.get_root_with_updates(&changeset);

        let mut updated_storage = harness.storage().clone();
        updated_storage.insert(key_2, updated_val);

        let updated_hashed = MockHashedCursorFactory::new(
            BTreeMap::new(),
            std::iter::once((harness.hashed_address(), updated_storage)).collect(),
        );

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_2));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = updated_hashed.hashed_storage_cursor(harness.hashed_address()).unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());
        let root_node = calculator
            .storage_root_node(harness.hashed_address())
            .expect("storage_root_node should succeed");

        let got_root = calculator
            .compute_root_hash(&[root_node])
            .expect("root hash should succeed")
            .expect("root should get hashed");
        pretty_assertions::assert_eq!(expected_root, got_root);
    }

    #[test]
    fn test_cached_hash_with_deleted_leaf() {
        reth_tracing::init_test_tracing();

        // Use different values to ensure distinct leaf hashes.
        let val_3 = U256::from(111u64);
        let val_5 = U256::from(222u64);
        let val_8 = U256::from(333u64);

        // Keys under a common prefix `0x6_` to create a branch at path [6].
        // Use second byte to distinguish short keys (so they differ after position 2).
        let key_63 = B256::right_padding_from(&[0x63, 0xaa]); // nibble path: 6,3,a,a,...
        let key_65 = B256::right_padding_from(&[0x65, 0xbb]); // nibble path: 6,5,b,b,...
        let key_68 = B256::right_padding_from(&[0x68, 0xcc]); // nibble path: 6,8,c,c,...

        // Compute leaf hashes. The branch at [6] consumes 2 nibbles (the branch path [6]
        // plus the child nibble), so each leaf's short key starts at position 2.
        let leaf_hash_3 = storage_leaf_hash(&Nibbles::unpack(key_63).slice(2..), &val_3);
        let leaf_hash_5 = storage_leaf_hash(&Nibbles::unpack(key_65).slice(2..), &val_5);
        let leaf_hash_8 = storage_leaf_hash(&Nibbles::unpack(key_68).slice(2..), &val_8);

        // Build cached branch at [6] with state_mask and hash_mask bits for nibbles 3, 5, 8.
        let state_mask = TrieMask::new((1 << 3) | (1 << 5) | (1 << 8));
        let cached_branch = BranchNodeCompact::new(
            state_mask,
            TrieMask::new(0),
            state_mask, // hash_mask = state_mask (all children have cached hashes)
            vec![leaf_hash_3, leaf_hash_5, leaf_hash_8],
            None,
        );

        let storage_nodes: BTreeMap<Nibbles, BranchNodeCompact> =
            std::iter::once((Nibbles::from_nibbles([0x6]), cached_branch)).collect();

        // Compute the expected root from a fresh trie with just key_65 and key_68.
        let mut harness =
            TrieTestHarness::new([(key_65, val_5), (key_68, val_8)].into_iter().collect());
        let expected_root = harness.original_root();

        // Update the harness with a cached trie node which will reference key_63 by hash.
        harness.set_trie_nodes(storage_nodes);

        // Mark key_63 as dirty in the prefix set — in the real scenario the leaf was
        // deleted and the HashedPostState overlay masks it out.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(key_63));

        // Request a proof for key_63 (absence proof — no leaf exists).
        // Because the prefix set marks nibble 3's child path as dirty, the cached hash for
        // nibble 3 is skipped.
        let mut targets = vec![ProofV2Target::new(key_63)];

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor)
            .with_prefix_set(prefix_set.freeze());

        let proofs = calculator
            .storage_proof(harness.hashed_address(), &mut targets)
            .expect("storage_proof should succeed");
        assert_eq!(1, proofs.len());
        let got_root = calculator
            .compute_root_hash(&proofs)
            .expect("compute_root_hash should succeed")
            .expect("should produce a root hash (proof contains root node)");

        // With the bug, nibble 5 gets hashes[0] (nibble 3's hash) and nibble 8 gets
        // hashes[1] (nibble 5's hash), producing a wrong root.
        pretty_assertions::assert_eq!(
            expected_root,
            got_root,
            "Root hash should match trie without key_63; cached hash index is off when \
             an earlier hashed child has no leaves (absence proof target)"
        );
    }

    #[test]
    fn test_exhausted_cursor_resets_at_equal_target_boundary() {
        let first = B256::ZERO;
        let last = B256::with_last_byte(1);
        let last_nibbles = Nibbles::unpack(last);
        let harness =
            ProofTestHarness::new(BTreeMap::from([(first, U256::from(1)), (last, U256::from(2))]));
        let mut targets = [
            ProofV2Target::new(first),
            ProofV2Target::new(last).with_parent(ProofV2TargetParent::new(63)),
        ];

        let (proof, root) = harness.proof_v2(&mut targets);

        assert_eq!(root, Some(harness.original_root()));
        let child = proof
            .iter()
            .find(|node| node.path == last_nibbles)
            .expect("depth-63 target child proof");
        let TrieNodeV2::Leaf(leaf) = &child.node else { panic!("target child should be a leaf") };
        assert!(leaf.key.is_empty());
    }

    #[test]
    fn test_prefix_set_root_proof_preserves_clean_sibling_after_cached_branch_collapse() {
        reth_tracing::init_test_tracing();

        let dirty = B256::right_padding_from(&[0x10, 0x00, 0x10]);
        let clean_sibling = B256::right_padding_from(&[0x10, 0x10]);
        let storage = [
            B256::right_padding_from(&[0x10]),
            dirty,
            B256::right_padding_from(&[0x10, 0x01]),
            B256::right_padding_from(&[0x10, 0x02]),
            clean_sibling,
            B256::right_padding_from(&[0x11]),
            B256::right_padding_from(&[0x12]),
        ]
        .into_iter()
        .map(|key| (key, U256::from(1u64)))
        .collect();

        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        let mut prefix_set_with_sibling = PrefixSetMut::default();
        prefix_set_with_sibling.insert(Nibbles::unpack(dirty));
        prefix_set_with_sibling.insert(Nibbles::unpack(clean_sibling));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set_with_sibling.freeze()),
        );
        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
            "a dirty prefix must not omit a clean sibling after collapsing a cached branch",
        );
    }

    #[test]
    fn test_prefix_set_range_skips_covered_cached_branch() {
        reth_tracing::init_test_tracing();

        let before = B256::right_padding_from(&[0x30]);
        let cached_a = B256::right_padding_from(&[0x80, 0x10]);
        let cached_b = B256::right_padding_from(&[0x80, 0x15]);
        let dirty = B256::right_padding_from(&[0x80, 0xc0]);
        let after = B256::right_padding_from(&[0x90]);

        // The cached branch at 0x80 remains parked while its children are rebuilt. Processing the
        // dirty child advances the lower bound to 0x9, past the cached branch's entire range.
        let storage = [before, cached_a, cached_b, dirty, after]
            .into_iter()
            .enumerate()
            .map(|(i, key)| (key, U256::from(i + 1)))
            .collect();

        let harness = ProofTestHarness::new(storage);
        let expected_root = harness.original_root();
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(dirty));

        pretty_assertions::assert_eq!(
            Some(expected_root),
            harness.root_with_prefix_set(prefix_set.freeze()),
        );
    }

    #[test]
    fn test_blinded_local_root_returns_trie_inconsistency() {
        let key = B256::right_padding_from(&[0x63, 0xaa]);
        let value = U256::from(1);
        let hash = storage_leaf_hash(&Nibbles::unpack(key).slice(2..), &value);
        let mask = TrieMask::from_nibble(3);
        let cached_branch =
            BranchNodeCompact::new(mask, TrieMask::default(), mask, vec![hash], None);

        let mut harness = TrieTestHarness::new(BTreeMap::from([(key, value)]));
        harness.set_trie_nodes(BTreeMap::from([(Nibbles::from_nibbles([6]), cached_branch)]));

        let trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut calculator = StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

        assert!(matches!(
            calculator.storage_root_node(harness.hashed_address()),
            Err(StateProofError::TrieInconsistency(_))
        ));
    }
}
