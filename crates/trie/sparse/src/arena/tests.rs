//! Arena implementation regression and property tests.

use super::TRACE_TARGET;
use crate::{
    ArenaParallelSparseTrie, ArenaParallelismThresholds, LeafUpdate, SparseTrie, TrieNodeEpoch,
};
use alloy_primitives::{map::B256Map, B256, U256};
use rand::{seq::SliceRandom, Rng, SeedableRng};
use reth_trie::test_utils::TrieTestHarness;
use reth_trie_common::ProofV2Target;
use std::collections::BTreeMap;
use tracing::{info, trace};

const fn epoch(value: u64) -> TrieNodeEpoch {
    TrieNodeEpoch::new(value)
}

#[test]
fn pruning_pops_cursor_before_reusing_node_slot() {
    use super::{
        ArenaCursor, ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeState, BranchChild,
        NextResult, NodeArena,
    };
    use reth_trie_common::RlpNode;

    let mut arena = NodeArena::new();
    let rlp = RlpNode::word_rlp(&B256::repeat_byte(1));
    let state = ArenaSparseNodeState::Cached { rlp_node: rlp.clone(), epoch: epoch(0) };
    let leaf =
        ArenaSparseNode::Leaf { state: state.clone(), key: Default::default(), value: vec![1] };
    let first = arena.insert(leaf.clone());
    let second = arena.insert(leaf);
    let root = arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
        state: state.clone(),
        children: [BranchChild::revealed(first), BranchChild::revealed(second)]
            .into_iter()
            .collect(),
        state_mask: alloy_trie::TrieMask::new(3),
        short_key: Default::default(),
        branch_masks: Default::default(),
    }));
    let mut cursor = ArenaCursor::default();
    cursor.reset(&arena, root, Default::default());
    assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::NonBranch));
    assert_eq!(cursor.head().unwrap().index, first);

    ArenaParallelSparseTrie::remove_pruned_node(&mut arena, &mut cursor);
    assert_eq!(cursor.head().unwrap().index, root);
    let reused = arena.insert(ArenaSparseNode::Leaf {
        state: ArenaSparseNodeState::Dirty,
        key: Default::default(),
        value: vec![2],
    });
    assert_eq!(reused, first);

    assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::NonBranch));
    assert_eq!(cursor.head().unwrap().index, second);
    assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::Branch));
    assert_eq!(arena[root].state_ref(), Some(&state));
    assert_eq!(arena.blinded(arena[root].branch_ref().children[0]), &rlp);
    assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::Done));
}

/// Test harness for proptest-based arena sparse trie testing.
///
/// Wraps [`TrieTestHarness`] and adds `ArenaParallelSparseTrie`-specific helpers for
/// the reveal-update loop and asserting that sparse trie updates match `StorageRoot`.
struct ArenaTrieTestHarness {
    /// The inner general-purpose harness.
    inner: TrieTestHarness,
}

impl std::ops::Deref for ArenaTrieTestHarness {
    type Target = TrieTestHarness;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for ArenaTrieTestHarness {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl ArenaTrieTestHarness {
    /// Creates a new test harness from a map of hashed storage slots to values.
    fn new(storage: BTreeMap<B256, U256>) -> Self {
        Self { inner: TrieTestHarness::new(storage) }
    }

    /// Computes the new storage root and trie updates after applying the given changes
    /// using both `StorageRoot` and the provided `ArenaParallelSparseTrie`, then asserts
    /// they match.
    fn assert_changes(&self, apst: &mut ArenaParallelSparseTrie, changes: BTreeMap<B256, U256>) {
        // Compute expected root and trie updates via StorageRoot.
        let (expected_root, mut expected_trie_updates) = if changes.is_empty() {
            (self.original_root(), Default::default())
        } else {
            self.get_root_with_updates(&changes)
        };

        self.minimize_trie_updates(&mut expected_trie_updates);

        // Build leaf updates for the APST: non-zero values are upserts (RLP-encoded),
        // zero values are deletions (empty vec).
        let mut leaf_updates: B256Map<LeafUpdate> = changes
            .iter()
            .map(|(&slot, &value)| {
                let rlp_value = if value.is_zero() {
                    Vec::new()
                } else {
                    alloy_rlp::encode_fixed_size(&value).to_vec()
                };
                (slot, LeafUpdate::Changed(rlp_value))
            })
            .collect();

        // Reveal-update loop: call update_leaves, collect required proofs, fetch them,
        // reveal, and repeat until no more proofs are needed.
        loop {
            let mut targets: Vec<ProofV2Target> = Vec::new();
            apst.update_leaves(&mut leaf_updates, |key, parent| {
                targets.push(ProofV2Target::new(key).with_parent(parent));
            })
            .expect("update_leaves should succeed");

            if targets.is_empty() {
                break;
            }

            let (mut proof_nodes, _) = self.proof_v2(&mut targets);
            apst.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
        }

        // Compute root and take updates from the APST.
        let actual_root = apst.root(epoch(0));
        let mut actual_updates = apst.take_updates();

        // Minimize sparse updates inline (can't use TrieTestHarness::minimize_sparse_updates
        // due to the crate's SparseTrieUpdates being a different type than reth-trie's copy).
        actual_updates
            .updated_nodes
            .retain(|path, node| self.storage_trie_updates().storage_nodes.get(path) != Some(node));
        actual_updates
            .removed_nodes
            .retain(|path| self.storage_trie_updates().storage_nodes.contains_key(path));

        let mut expected_updated_nodes =
            expected_trie_updates.storage_nodes.into_iter().collect::<Vec<_>>();
        let mut actual_updated_nodes = actual_updates.updated_nodes.into_iter().collect::<Vec<_>>();
        expected_updated_nodes.sort();
        actual_updated_nodes.sort();
        pretty_assertions::assert_eq!(
            expected_updated_nodes,
            actual_updated_nodes,
            "updated nodes mismatch"
        );

        let mut expected_removed_nodes =
            expected_trie_updates.removed_nodes.into_iter().collect::<Vec<_>>();
        let mut actual_removed_nodes = actual_updates.removed_nodes.into_iter().collect::<Vec<_>>();
        expected_removed_nodes.sort();
        actual_removed_nodes.sort();
        pretty_assertions::assert_eq!(
            expected_removed_nodes,
            actual_removed_nodes,
            "removed nodes mismatch"
        );
        assert_eq!(expected_root, actual_root, "storage root mismatch");
    }
}

use proptest::prelude::*;
use proptest_arbitrary_interop::arb;

/// Builds a changeset by mixing `new_keys` (fresh insertions) with a fraction of
/// existing keys from `base` (updates/deletions).
///
/// `overlap_pct` controls how many existing keys are included, and `delete_pct`
/// controls how many of those become deletions (zero values). The remaining
/// overlap keys get random non-zero values.
fn build_changeset(
    base: &BTreeMap<B256, U256>,
    new_keys: BTreeMap<B256, U256>,
    overlap_pct: f64,
    delete_pct: f64,
    rng: &mut rand::rngs::StdRng,
) -> BTreeMap<B256, U256> {
    let num_overlap = (base.len() as f64 * overlap_pct) as usize;
    let num_delete = (num_overlap as f64 * delete_pct) as usize;

    let mut all_keys: Vec<B256> = base.keys().copied().collect();
    all_keys.shuffle(rng);
    let overlap_keys = &all_keys[..num_overlap];

    let mut changeset = new_keys;
    for (i, &key) in overlap_keys.iter().enumerate() {
        let value = if i < num_delete { U256::ZERO } else { U256::from(rng.random::<u64>() | 1) };
        changeset.entry(key).or_insert(value);
    }
    changeset
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]
    #[test]
    fn arena_trie_proptest(
        initial in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=100usize),
        changeset1_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
        changeset2_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
        overlap_pct in 0.0..=0.5f64,
        delete_pct in 0.0..=0.33f64, // percent of overlapping changeset which are deletes
        shuffle_seed in arb::<u64>(),
    ) {
        reth_tracing::init_test_tracing();
        info!(target: TRACE_TARGET, ?shuffle_seed, "PROPTEST START");

        // Filter out zero-valued entries from the initial dataset (zeros mean "absent").
        let initial: BTreeMap<B256, U256> = initial.into_iter()
            .filter(|(_, v)| !v.is_zero())
            .collect();

        let mut rng = rand::rngs::StdRng::seed_from_u64(shuffle_seed);

        let changeset1 = build_changeset(&initial, changeset1_new_keys, overlap_pct, delete_pct, &mut rng);
        for (i, (k, v)) in changeset1.iter().enumerate() {
            trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 1 entry");
        }

        let mut harness = ArenaTrieTestHarness::new(initial);

        // Initialize the APST from the harness root node.
        let root_node = harness.root_node();
        let mut apst = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
            ArenaParallelismThresholds {
                min_dirty_leaves: 3,
                min_revealed_nodes: 3,
                min_updates: 3,
                min_leaves_for_prune: 3,
            },
        );
        apst.set_root(root_node.node, root_node.masks, true).expect("set_root should succeed");

        harness.assert_changes(&mut apst, changeset1.clone());

        // Update the harness base dataset to reflect the first changeset.
        harness.apply_changeset(changeset1);

        // All nodes were cached at epoch 0, so this maximally prunes the trie before the
        // second update round.
        apst.prune(epoch(1));

        let changeset2 = build_changeset(harness.storage(), changeset2_new_keys, overlap_pct, delete_pct, &mut rng);
        for (i, (k, v)) in changeset2.iter().enumerate() {
            trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 2 entry");
        }

        harness.assert_changes(&mut apst, changeset2);
    }
}
