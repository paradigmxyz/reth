use super::*;

/// Branch root initializes correctly and produces correct hash.
///
/// Two leaves whose first nibbles differ produce a branch root node.
/// After `set_root` + `reveal_nodes`, `root()` must match the reference hash.
pub(super) fn test_set_root_with_branch_node<T: SparseTrie + Default>() {
    // Keys whose first nibbles differ → branch at root.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10; // first nibble = 1
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20; // first nibble = 2
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);
    let root = trie.root();
    assert_eq!(root, harness.original_root());
}

/// Single-leaf root initializes correctly.
///
/// A single key-value pair produces a leaf root node. After `set_root` + `root()`,
/// the hash must match the reference trie.
pub(super) fn test_set_root_with_leaf_node<T: SparseTrie + Default>() {
    let storage: BTreeMap<B256, U256> = BTreeMap::from([(B256::ZERO, U256::from(42))]);

    let harness = SuiteTestHarness::new(storage);
    let root_node = harness.root_node();
    let mut trie = T::default();
    trie.set_root(root_node.node, root_node.masks, true).expect("set_root should succeed");
    let root = trie.root();
    assert_eq!(root, harness.original_root());
}

/// Extension root (shared prefix) initializes correctly.
///
/// Two keys sharing a long common prefix produce an extension root node.
/// After `set_root` + `reveal_nodes`, `root()` must match the reference hash.
pub(super) fn test_set_root_with_extension_node<T: SparseTrie + Default>() {
    // Keys that share first byte 0xAB → extension root.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0xAB;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0xAB;
    key_b.0[31] = 0x01;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);
    let root = trie.root();
    assert_eq!(root, harness.original_root());
}

/// `retain_updates=true` enables update tracking.
///
/// When `set_root` is called with `retain_updates = true`, subsequent mutations
/// should be tracked and `take_updates()` should return non-empty results.
pub(super) fn test_set_root_retains_updates_when_requested<T: SparseTrie + Default>() {
    // Build a trie with enough leaves to produce non-root branch nodes with hash children.
    // We need leaves sharing a prefix nibble so that intermediate branch nodes are created,
    // and enough entries that children are hashed (RLP ≥ 32 bytes).
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10; // shared first nibble → branch at nibble 1
        key.0[1] = i * 16; // differ in second nibble → branch below
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root once so branch hashes are cached.
    let _ = trie.root();

    // Add a new leaf under the same prefix so non-root branch nodes change.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    let changeset: BTreeMap<B256, U256> = BTreeMap::from([(new_key, U256::from(100))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // Compute root to finalize hashes and generate update actions.
    let _ = trie.root();

    // take_updates should return non-empty updates.
    let updates = trie.take_updates();
    assert!(
        !updates.updated_nodes.is_empty() || !updates.removed_nodes.is_empty(),
        "take_updates should be non-empty when retain_updates=true: updated={}, removed={}",
        updates.updated_nodes.len(),
        updates.removed_nodes.len(),
    );
}

/// `retain_updates=false` disables update tracking.
///
/// When `set_root` is called with `retain_updates = false`, `take_updates()` should
/// return an empty `SparseTrieUpdates` even after mutations.
pub(super) fn test_set_root_does_not_retain_updates_when_not_requested<T: SparseTrie + Default>() {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    // retain_updates = false
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Modify a leaf.
    let changeset: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::from(99))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root();

    let updates = trie.take_updates();
    assert!(
        updates.updated_nodes.is_empty() && updates.removed_nodes.is_empty(),
        "take_updates should be empty when retain_updates=false: updated={}, removed={}",
        updates.updated_nodes.len(),
        updates.removed_nodes.len(),
    );
}

/// `EmptyRoot` produces `EMPTY_ROOT_HASH`.
///
/// Setting the root to `TrieNodeV2::EmptyRoot` should leave the trie in its initial
/// empty state, returning `EMPTY_ROOT_HASH` from `root()`.
pub(super) fn test_set_root_with_empty_root<T: SparseTrie + Default>() {
    let mut trie = T::default();
    trie.set_root(TrieNodeV2::EmptyRoot, None, true).expect("set_root should succeed");
    assert_eq!(trie.root(), EMPTY_ROOT_HASH);
}
