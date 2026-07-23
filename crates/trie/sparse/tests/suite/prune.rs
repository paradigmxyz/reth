use super::*;
use alloy_trie::{nodes::BranchNode, TrieMask};
use reth_trie_common::{BranchNodeV2, LeafNode, ProofTrieNodeV2, RlpNode};

fn key_with_prefix(prefix: &[u8]) -> B256 {
    let mut key = B256::ZERO;
    for (idx, &nibble) in prefix.iter().enumerate() {
        let byte = &mut key.0[idx / 2];
        if idx % 2 == 0 {
            *byte |= nibble << 4;
        } else {
            *byte |= nibble;
        }
    }
    key
}

fn changed_update(value: u64) -> LeafUpdate {
    LeafUpdate::Changed(encode_fixed_size(&U256::from(value)).to_vec())
}

fn assert_update_requests_parent<T: SparseTrie>(
    trie: &mut T,
    key: B256,
    parent: ProofV2TargetParent,
    value: u64,
) {
    let mut leaf_updates = B256Map::from_iter([(key, changed_update(value))]);
    let mut targets = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, parent| {
        targets.push((key, parent));
    })
    .expect("update_leaves should succeed");

    assert_eq!(targets, vec![(key, parent)]);
    assert!(
        leaf_updates.contains_key(&key),
        "update should remain pending until proof is revealed"
    );
}

fn rlp_node(node: TrieNodeV2) -> RlpNode {
    RlpNode::from_rlp(&alloy_rlp::encode(node))
}

pub(super) fn test_prune_retains_specified_leaves<T: SparseTrie>(new_trie: fn() -> T) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let mut key_d = B256::ZERO;
    key_d.0[0] = 0x40;
    let mut key_e = B256::ZERO;
    key_e.0[0] = 0x50;

    let storage: BTreeMap<B256, U256> = BTreeMap::from([
        (key_a, U256::from(1)),
        (key_b, U256::from(2)),
        (key_c, U256::from(3)),
        (key_d, U256::from(4)),
        (key_e, U256::from(5)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    // Compute root before prune.
    let hash1 = trie.root(0);

    // Retain leaves A and B, prune the rest.
    let retained = [Nibbles::unpack(key_a), Nibbles::unpack(key_b)];
    trie.prune(&retained);

    // Root must be unchanged after prune.
    let hash2 = trie.root(0);
    assert_eq!(hash1, hash2, "root hash should be unchanged after prune");

    // Retained leaves must still be accessible.
    let val_a = trie.get_leaf_value(&Nibbles::unpack(key_a));
    assert!(val_a.is_some(), "retained leaf A should be accessible after prune");

    let val_b = trie.get_leaf_value(&Nibbles::unpack(key_b));
    assert!(val_b.is_some(), "retained leaf B should be accessible after prune");
}

pub(super) fn test_prune_keeps_upper_children_of_retained_branch<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let retained_key = key_with_prefix(&[0x0]);
    let protected_key_a = key_with_prefix(&[0x1, 0x2]);
    let protected_key_b = key_with_prefix(&[0x1, 0x3]);
    let storage = BTreeMap::from([
        (retained_key, U256::from(1)),
        (protected_key_a, U256::from(2)),
        (protected_key_b, U256::from(3)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    let root_before = trie.root(0);

    let retained = [Nibbles::unpack(retained_key)];
    let pruned = trie.prune(&retained);

    assert_eq!(trie.root(0), root_before, "root must not change after prune");
    assert_eq!(pruned, 2, "protected branch children should be blinded, not removed");
    assert_update_requests_parent(&mut trie, protected_key_a, ProofV2TargetParent::new(1), 20);
}

pub(super) fn test_prune_keeps_lower_children_of_retained_branch<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let retained_key = key_with_prefix(&[0x0, 0x0, 0x0]);
    let protected_key_a = key_with_prefix(&[0x0, 0x0, 0x1, 0x2]);
    let protected_key_b = key_with_prefix(&[0x0, 0x0, 0x1, 0x3]);
    let storage = BTreeMap::from([
        (retained_key, U256::from(1)),
        (protected_key_a, U256::from(2)),
        (protected_key_b, U256::from(3)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    let root_before = trie.root(0);

    let retained = [Nibbles::unpack(retained_key)];
    let pruned = trie.prune(&retained);

    assert_eq!(trie.root(0), root_before, "root must not change after prune");
    assert_eq!(pruned, 2, "protected lower branch children should be blinded, not removed");
    assert_update_requests_parent(&mut trie, protected_key_a, ProofV2TargetParent::new(3), 20);
}

pub(super) fn test_prune_protects_children_by_parent_base_path<T: SparseTrie>(new_trie: fn() -> T) {
    let protected_leaf_a = TrieNodeV2::Leaf(LeafNode::new(Nibbles::default(), vec![0x32]));
    let protected_leaf_b = TrieNodeV2::Leaf(LeafNode::new(Nibbles::default(), vec![0x33]));

    let protected_state_mask = TrieMask::new(0b1100);
    let protected_stack =
        vec![rlp_node(protected_leaf_a.clone()), rlp_node(protected_leaf_b.clone())];
    let protected_branch_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
        protected_stack.clone(),
        protected_state_mask,
    )));
    let retained_parent = TrieNodeV2::Branch(BranchNodeV2::new(
        Nibbles::from_nibbles([0xa]),
        protected_stack,
        protected_state_mask,
        Some(protected_branch_rlp),
    ));
    let root = TrieNodeV2::Branch(BranchNodeV2::new(
        Nibbles::default(),
        vec![rlp_node(retained_parent.clone())],
        TrieMask::new(0b0001),
        None,
    ));

    let mut trie = (new_trie)();
    trie.set_root(root, None, false).expect("set_root should succeed");
    trie.reveal_nodes(&mut [
        ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x0]), node: retained_parent, masks: None },
        ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x0, 0xa, 0x2]),
            node: protected_leaf_a,
            masks: None,
        },
        ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x0, 0xa, 0x3]),
            node: protected_leaf_b,
            masks: None,
        },
    ])
    .expect("reveal_nodes should succeed");

    let root_before = trie.root(0);
    let retained = [Nibbles::from_nibbles([0x0, 0x0])];
    let pruned = trie.prune(&retained);

    assert_eq!(pruned, 0);
    assert_eq!(trie.root(0), root_before, "root must not change after prune");
}

/// Pruning should reduce the node count.
///
/// Build a trie with several root children that each contain grandchildren, fully reveal
/// and compute root. Then prune retaining only 1 leaf. `size_hint()` must
/// decrease and `prune` must return > 0.
pub(super) fn test_prune_reduces_node_count<T: SparseTrie>(new_trie: fn() -> T) {
    // Create 16 pairs with different first nibbles. Pruning keeps direct
    // children of the retained root branch, so each child needs grandchildren that can be pruned.
    let keys: Vec<B256> = (0u8..16)
        .flat_map(|i| {
            [0u8, 1].map(move |child| {
                let mut k = B256::ZERO;
                k.0[0] = (i << 4) | child;
                k
            })
        })
        .collect();

    let storage: BTreeMap<B256, U256> =
        keys.iter().enumerate().map(|(i, k)| (*k, U256::from(i + 1))).collect();

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    // Compute root to cache hashes (required for pruning).
    let _root = trie.root(0);

    let size_before = trie.size_hint();

    // Retain only the first leaf.
    let retained = [Nibbles::unpack(keys[0])];
    let pruned_count = trie.prune(&retained);

    let size_after = trie.size_hint();

    assert!(pruned_count > 0, "prune should convert at least one node to a stub");
    assert!(
        size_after < size_before,
        "size_hint should decrease after prune: before={size_before}, after={size_after}"
    );
}

/// Pruning with an empty retained set should convert all subtrees to
/// hash stubs (maximum pruning). Root hash must be unchanged.
pub(super) fn test_prune_empty_retained_set<T: SparseTrie>(new_trie: fn() -> T) {
    let keys: Vec<B256> = (0u8..16)
        .map(|i| {
            let mut k = B256::ZERO;
            k.0[0] = (i + 1) << 4;
            k
        })
        .collect();

    let storage: BTreeMap<B256, U256> =
        keys.iter().enumerate().map(|(i, k)| (*k, U256::from(i + 1))).collect();

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let hash_before = trie.root(0);

    let size_before = trie.size_hint();

    // Prune with empty retained set — maximum pruning.
    let pruned_count = trie.prune(&[]);

    let hash_after = trie.root(0);
    let size_after = trie.size_hint();

    assert_eq!(hash_before, hash_after, "root hash should be unchanged after prune");
    assert!(pruned_count > 0, "prune should convert at least one node to a stub");
    assert!(
        size_after < size_before,
        "size_hint should decrease after max prune: before={size_before}, after={size_after}"
    );
}

pub(super) fn test_prune_requires_computed_hashes<T: SparseTrie>(new_trie: fn() -> T) {
    let keys: Vec<B256> = (0u8..5)
        .map(|i| {
            let mut k = B256::ZERO;
            k.0[0] = (i + 1) << 4;
            k
        })
        .collect();

    let storage: BTreeMap<B256, U256> =
        keys.iter().enumerate().map(|(i, k)| (*k, U256::from(i + 1))).collect();

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    // Dirty the trie by updating a leaf — do NOT call root() to compute hashes.
    let mut leaf_updates: B256Map<LeafUpdate> = B256Map::default();
    leaf_updates.insert(keys[0], LeafUpdate::Changed(encode_fixed_size(&U256::from(999)).to_vec()));
    trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");

    // Prune without having called root() — dirty nodes lack cached hashes.
    let retained = vec![Nibbles::unpack(keys[0])];
    let pruned_count = trie.prune(&retained);

    // Compare against pruning after root() is called (clean state).
    // With dirty nodes, pruning is limited because dirty subtrees lack cached hashes.
    let mut trie_clean: T = harness.init_trie_fully_revealed(false, new_trie);
    trie_clean.root(0);
    let clean_pruned = trie_clean.prune(&retained);

    assert!(
        pruned_count <= clean_pruned,
        "dirty prune ({pruned_count}) should not exceed clean prune ({clean_pruned})"
    );
}

pub(super) fn test_prune_then_update_and_recompute_root<T: SparseTrie>(new_trie: fn() -> T) {
    let keys: Vec<B256> = (0u8..5)
        .map(|i| {
            let mut k = B256::ZERO;
            k.0[0] = (i + 1) << 4;
            k
        })
        .collect();

    let storage: BTreeMap<B256, U256> =
        keys.iter().enumerate().map(|(i, k)| (*k, U256::from(i + 1))).collect();

    let harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    trie.root(0);

    let retained = vec![Nibbles::unpack(keys[0]), Nibbles::unpack(keys[1])];
    trie.prune(&retained);

    let new_value = U256::from(999);
    let mut leaf_updates: B256Map<LeafUpdate> = B256Map::default();
    leaf_updates.insert(keys[0], LeafUpdate::Changed(encode_fixed_size(&new_value).to_vec()));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root_after = trie.root(0);

    let mut expected_storage = storage;
    expected_storage.insert(keys[0], new_value);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    let expected_root = expected_harness.original_root();

    assert_eq!(root_after, expected_root, "root after prune + update should match reference trie");
}

pub(super) fn test_prune_then_reveal_pruned_subtree<T: SparseTrie>(new_trie: fn() -> T) {
    let keys: Vec<B256> = (0u8..5)
        .map(|i| {
            let mut k = B256::ZERO;
            k.0[0] = (i + 1) << 4;
            k
        })
        .collect();

    let storage: BTreeMap<B256, U256> =
        keys.iter().enumerate().map(|(i, k)| (*k, U256::from(i + 1))).collect();

    let harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    trie.root(0);

    let retained = vec![Nibbles::unpack(keys[0])];
    trie.prune(&retained);

    let new_value = U256::from(777);
    let mut leaf_updates: B256Map<LeafUpdate> = B256Map::default();
    leaf_updates.insert(keys[2], LeafUpdate::Changed(encode_fixed_size(&new_value).to_vec()));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root_after = trie.root(0);

    let mut expected_storage = storage;
    expected_storage.insert(keys[2], new_value);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    let expected_root = expected_harness.original_root();

    assert_eq!(
        root_after, expected_root,
        "root after prune + reveal pruned subtree + update should match reference trie"
    );
}

/// Pruning a trie with both large (hashed) and small (embedded) node values
/// should preserve the root hash.
pub(super) fn test_prune_mixed_embedded_and_hashed_nodes<T: SparseTrie>(new_trie: fn() -> T) {
    let mut storage = BTreeMap::new();

    // 4 keys with large values (produce hashed nodes: RLP ≥ 32 bytes)
    for i in 0..4u8 {
        let mut key = B256::ZERO;
        key.0[0] = i;
        storage.insert(key, U256::MAX);
    }
    // 4 keys with small values (produce embedded nodes: RLP < 32 bytes)
    for i in 4..8u8 {
        let mut key = B256::ZERO;
        key.0[0] = i;
        storage.insert(key, U256::from(1));
    }

    let mut trie = (new_trie)();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&storage);
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected on empty trie");
    })
    .expect("update_leaves should succeed");

    let root_before = trie.root(0);
    trie.prune(&[]);
    let root_after = trie.root(0);

    assert_eq!(root_before, root_after, "root hash must be preserved after pruning mixed trie");
}

/// After pruning, inserting a new leaf at a
/// previously-unrevealed path should not panic.
pub(super) fn test_prune_then_update_no_panic<T: SparseTrie>(new_trie: fn() -> T) {
    // Build a trie with 64 leaves (16 keys × 4 first-nibble groups).
    let mut storage = BTreeMap::new();
    for group in 0..4u8 {
        for i in 0..16u8 {
            let mut key = B256::ZERO;
            key.0[0] = (group << 4) | i;
            storage.insert(key, U256::from((group as u64) * 16 + i as u64 + 1));
        }
    }

    let harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let root_before_prune = trie.root(0);

    // Prune everything.
    trie.prune(&[]);

    let hash1 = trie.root(0);
    assert_eq!(hash1, root_before_prune, "root after prune must equal root before prune");

    // Insert a brand-new key not previously in the trie.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0xFF;
    let value_bytes = encode_fixed_size(&U256::from(999));
    let mut leaf_updates =
        B256Map::from_iter([(new_key, LeafUpdate::Changed(value_bytes.to_vec()))]);

    // The update will hit blinded nodes — the reveal_and_update loop supplies proofs.
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let hash2 = trie.root(0);
    assert_ne!(hash2, hash1, "root should change after inserting a new leaf");
}

/// When the root is not a branch (e.g., a single
/// leaf or empty root), `prune` should immediately return 0 without walking.
pub(super) fn test_prune_only_descends_into_branch_root<T: SparseTrie>(new_trie: fn() -> T) {
    // Single-leaf trie: root is a leaf node, not a branch.
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(B256::with_last_byte(0x10), U256::from(1))]);
    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let _root = trie.root(0);
    let pruned = trie.prune(&[]);
    assert_eq!(pruned, 0, "non-branch root should not prune any nodes");

    // Empty root: also not a branch.
    let mut empty_trie = (new_trie)();
    let pruned_empty = empty_trie.prune(&[]);
    assert_eq!(pruned_empty, 0, "empty root should not prune any nodes");
}

/// Small subtrie root nodes (RLP < 32 bytes) are
/// handled correctly during prune. After `root()` + `prune()`, a subsequent `root()`
/// still returns the same hash.
pub(super) fn test_prune_handles_small_subtrie_root_nodes<T: SparseTrie>(new_trie: fn() -> T) {
    // Build a trie with two groups of leaves to create a branch root with mixed
    // subtrie sizes:
    // - Group A (nibble 0x1): 16 leaves with large values → hashable subtrie root (RLP ≥ 32 bytes)
    // - Group B (nibble 0x2): 1 leaf with a small value → small subtrie root (RLP < 32 bytes)
    let mut storage = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | (i & 0x0F);
        // large value ensures the subtrie root RLP ≥ 32 bytes
        storage.insert(key, U256::MAX);
    }
    // Small subtrie: single small leaf
    let mut small_key = B256::ZERO;
    small_key.0[0] = 0x20;
    storage.insert(small_key, U256::from(1));

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let root_before = trie.root(0);
    assert_eq!(root_before, harness.original_root());

    // Prune retaining only the small-subtrie leaf — the large subtrie should
    // be replaced by hash stubs, and the small subtrie handled gracefully.
    let retained = vec![Nibbles::unpack(small_key)];
    trie.prune(&retained);

    let root_after = trie.root(0);
    assert_eq!(root_after, root_before, "root must not change after prune");
}
