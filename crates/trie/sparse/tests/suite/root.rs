use super::*;

/// Calling `root()` on a fresh, empty trie returns `EMPTY_ROOT_HASH`.
pub(super) fn test_root_empty_trie<T: SparseTrie>(new_trie: fn() -> T) {
    let mut trie = (new_trie)();
    assert_eq!(trie.root(), EMPTY_ROOT_HASH, "empty trie should return EMPTY_ROOT_HASH");
}

/// Second `root()` call returns cached hash without recomputation.
///
/// After fully revealing and computing root once, calling `root()` again without
/// mutations should return the same hash and `is_root_cached()` should be true.
pub(super) fn test_root_cached_returns_without_recomputation<T: SparseTrie>(new_trie: fn() -> T) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    let root1 = trie.root();
    assert_eq!(root1, harness.original_root(), "first root should match reference");

    assert!(trie.is_root_cached(), "root should be cached after first computation");

    let root2 = trie.root();
    assert_eq!(root2, root1, "second root call should return the same cached hash");
}

/// Single update changes root.
///
/// After modifying one leaf's value, `root()` should differ from the original and
/// match the reference trie with the updated value.
pub(super) fn test_root_after_single_leaf_update<T: SparseTrie>(new_trie: fn() -> T) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let original_root = trie.root();
    assert_eq!(original_root, harness.original_root(), "initial root should match reference");

    // Modify key_b's value from 2 to 999.
    let changes: BTreeMap<B256, U256> = BTreeMap::from([(key_b, U256::from(999))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changes);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let new_root = trie.root();
    assert_ne!(new_root, original_root, "root should change after leaf update");

    // Build a reference trie with the updated value and verify.
    let expected_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(999)), (key_c, U256::from(3))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        new_root,
        expected_harness.original_root(),
        "root should match updated reference trie"
    );
}

/// Insert order doesn't affect root hash.
///
/// Two tries built from the same 5 key-value pairs inserted in different orders
/// must produce the same root hash.
pub(super) fn test_root_deterministic_across_update_orders<T: SparseTrie>(new_trie: fn() -> T) {
    // Define 5 key-value pairs spread across different subtrie regions.
    let mut k1 = B256::ZERO;
    k1.0[0] = 0x10;
    let mut k2 = B256::ZERO;
    k2.0[0] = 0x20;
    let mut k3 = B256::ZERO;
    k3.0[0] = 0x30;
    let mut k4 = B256::ZERO;
    k4.0[0] = 0x40;
    let mut k5 = B256::ZERO;
    k5.0[0] = 0x50;

    let pairs = [
        (k1, U256::from(1)),
        (k2, U256::from(2)),
        (k3, U256::from(3)),
        (k4, U256::from(4)),
        (k5, U256::from(5)),
    ];

    // Build trie A: insert keys in order 1,2,3,4,5.
    let order_a = [pairs[0], pairs[1], pairs[2], pairs[3], pairs[4]];
    let root_a = {
        let mut trie = (new_trie)();
        for (key, value) in &order_a {
            let mut leaf_updates =
                SuiteTestHarness::leaf_updates(&BTreeMap::from([(*key, *value)]));
            trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");
        }
        trie.root()
    };

    // Build trie B: insert keys in order 5,3,1,4,2.
    let order_b = [pairs[4], pairs[2], pairs[0], pairs[3], pairs[1]];
    let root_b = {
        let mut trie = (new_trie)();
        for (key, value) in &order_b {
            let mut leaf_updates =
                SuiteTestHarness::leaf_updates(&BTreeMap::from([(*key, *value)]));
            trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");
        }
        trie.root()
    };

    assert_eq!(root_a, root_b, "root hash should be deterministic regardless of insert order");

    // Also verify against a reference trie built from sorted storage.
    let all_storage: BTreeMap<B256, U256> = pairs.iter().copied().collect();
    let harness = SuiteTestHarness::new(all_storage);
    assert_eq!(root_a, harness.original_root(), "root should match reference trie");
}

/// Small RLP root (<32 bytes) handled correctly.
///
/// When the root node's RLP encoding is smaller than 32 bytes, `root()` must still return the
/// correct hash. A second `root()` call should use the cached result without panic.
pub(super) fn test_root_handles_small_root_node_without_hash<T: SparseTrie>(new_trie: fn() -> T) {
    // A single small leaf produces a root node whose RLP is < 32 bytes.
    let key = B256::with_last_byte(1);
    let value = U256::from(1);

    let storage: BTreeMap<B256, U256> = BTreeMap::from([(key, value)]);
    let harness = SuiteTestHarness::new(storage);

    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let root1 = trie.root();
    assert_eq!(root1, harness.original_root(), "first root() should match reference trie");

    let root2 = trie.root();
    assert_eq!(root2, root1, "second root() should return cached result without panic");
}
