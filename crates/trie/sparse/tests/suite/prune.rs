use super::*;

pub(super) fn test_prune_retains_recent_leaves<T: SparseTrie>(new_trie: fn() -> T) {
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

    let initial_root = trie.root(0);
    assert_eq!(trie.prune(0), 0, "the epoch cutoff must be strict");
    assert_eq!(trie.root(0), initial_root);

    let mut updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([
        (key_a, U256::from(10)),
        (key_b, U256::from(20)),
    ]));
    trie.update_leaves(&mut updates, |_, _| panic!("fully revealed trie must not request proofs"))
        .expect("update_leaves should succeed");
    assert!(updates.is_empty());

    let root_before = trie.root(2);
    assert!(trie.prune(2) > 0);
    assert_eq!(trie.root(3), root_before, "root hash should be unchanged after prune");

    assert!(
        trie.get_leaf_value(&Nibbles::unpack(key_a)).is_some(),
        "leaf at the cutoff epoch should remain accessible"
    );
    assert!(
        trie.get_leaf_value(&Nibbles::unpack(key_b)).is_some(),
        "leaf at the cutoff epoch should remain accessible"
    );
    assert!(
        trie.get_leaf_value(&Nibbles::unpack(key_c)).is_none(),
        "leaf older than the cutoff should be blinded"
    );
}

pub(super) fn test_prune_retains_structurally_modified_branch<T: SparseTrie>(new_trie: fn() -> T) {
    let key = |first_byte, second_byte| {
        let mut key = B256::ZERO;
        key.0[0] = first_byte;
        key.0[1] = second_byte;
        key
    };
    let old_keys = [key(0x00, 0x00), key(0x00, 0x10), key(0x00, 0x20)];
    let sibling = key(0x01, 0x00);
    let mut storage = BTreeMap::from([
        (old_keys[0], U256::from(1)),
        (old_keys[1], U256::from(2)),
        (old_keys[2], U256::from(3)),
        (sibling, U256::from(4)),
    ]);

    let mut trie = (new_trie)();
    let mut initial_updates = SuiteTestHarness::leaf_updates(&storage);
    trie.update_leaves(&mut initial_updates, |_, _| panic!("empty trie must not request proofs"))
        .expect("update_leaves should succeed");
    assert!(initial_updates.is_empty());
    trie.root(10);

    let mut deletion = SuiteTestHarness::leaf_updates(&BTreeMap::from([(old_keys[2], U256::ZERO)]));
    trie.update_leaves(&mut deletion, |_, _| panic!("fully revealed trie must not request proofs"))
        .expect("update_leaves should succeed");
    assert!(deletion.is_empty());
    storage.remove(&old_keys[2]);

    let root_before = trie.root(20);
    assert_eq!(root_before, SuiteTestHarness::new(storage.clone()).original_root());
    trie.prune(20);
    assert_eq!(trie.root(20), root_before, "root hash should be unchanged after prune");

    let inserted = key(0x00, 0x30);
    let mut insertion =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(inserted, U256::from(5))]));
    trie.update_leaves(&mut insertion, |_, _| {
        panic!("structurally modified branch at the cutoff epoch must remain revealed")
    })
    .expect("update_leaves should succeed");
    assert!(insertion.is_empty());

    storage.insert(inserted, U256::from(5));
    assert_eq!(trie.root(21), SuiteTestHarness::new(storage).original_root());
}

/// Pruning should reduce the node count.
///
/// Build a trie with several root children that each contain grandchildren, fully reveal,
/// compute the root, and prune all nodes older than epoch 1.
pub(super) fn test_prune_reduces_node_count<T: SparseTrie>(new_trie: fn() -> T) {
    // Create 16 pairs with different first nibbles so pruning exercises nested branches.
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
    let root_before = trie.root(0);

    let pruned_count = trie.prune(1);

    assert_eq!(trie.root(1), root_before, "root hash should be unchanged after prune");
    assert!(pruned_count > 0, "prune should convert at least one node to a stub");
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

    let mut first_update = B256Map::from_iter([(
        keys[0],
        LeafUpdate::Changed(encode_fixed_size(&U256::from(100)).to_vec()),
    )]);
    trie.update_leaves(&mut first_update, |_, _| {
        panic!("fully revealed trie must not request proofs")
    })
    .expect("update_leaves should succeed");
    assert!(first_update.is_empty());
    trie.root(2);
    trie.prune(2);

    let new_value = U256::from(999);
    let mut leaf_updates = B256Map::from_iter([(
        keys[0],
        LeafUpdate::Changed(encode_fixed_size(&new_value).to_vec()),
    )]);
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("leaf at the cutoff epoch should remain revealed")
    })
    .expect("update_leaves should succeed");
    assert!(leaf_updates.is_empty());

    let root_after = trie.root(3);

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
    trie.prune(1);

    let new_value = U256::from(777);
    let mut leaf_updates: B256Map<LeafUpdate> = B256Map::default();
    leaf_updates.insert(keys[2], LeafUpdate::Changed(encode_fixed_size(&new_value).to_vec()));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root_after = trie.root(2);

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
    trie.prune(1);
    let root_after = trie.root(1);

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

    // Prune everything older than epoch 1.
    trie.prune(1);

    let hash1 = trie.root(1);
    assert_eq!(hash1, root_before_prune, "root after prune must equal root before prune");

    // Insert a brand-new key not previously in the trie.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0xFF;
    let value_bytes = encode_fixed_size(&U256::from(999));
    let mut leaf_updates =
        B256Map::from_iter([(new_key, LeafUpdate::Changed(value_bytes.to_vec()))]);

    // The update will hit blinded nodes — the reveal_and_update loop supplies proofs.
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let hash2 = trie.root(2);
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
    let pruned = trie.prune(1);
    assert_eq!(pruned, 0, "non-branch root should not prune any nodes");

    // Empty root: also not a branch.
    let mut empty_trie = (new_trie)();
    empty_trie.root(0);
    let pruned_empty = empty_trie.prune(1);
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
    let mut large_key = B256::ZERO;
    large_key.0[0] = 0x10;
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

    trie.root(0);
    let mut update = B256Map::from_iter([(
        small_key,
        LeafUpdate::Changed(encode_fixed_size(&U256::from(2)).to_vec()),
    )]);
    trie.update_leaves(&mut update, |_, _| panic!("fully revealed trie must not request proofs"))
        .expect("update_leaves should succeed");
    assert!(update.is_empty());

    let root_before = trie.root(2);
    trie.prune(2);

    let root_after = trie.root(2);
    assert_eq!(root_after, root_before, "root must not change after prune");
    assert!(trie.get_leaf_value(&Nibbles::unpack(small_key)).is_some());
    assert!(
        trie.get_leaf_value(&Nibbles::unpack(large_key)).is_none(),
        "old large subtrie should be blinded"
    );
}
