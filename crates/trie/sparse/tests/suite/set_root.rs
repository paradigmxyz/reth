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

/// Branch masks influence `removed_nodes` detection.
///
/// When proof nodes carry `BranchNodeMasks`, `reveal_nodes` registers them in the
/// trie's internal `branch_node_masks`. After mutations that remove leaves (causing
/// branch nodes to disappear), `take_updates().removed_nodes` should correctly
/// report nodes whose masks were previously registered but are now gone.
pub(super) fn test_set_root_with_branch_masks<T: SparseTrie + Default>() {
    // 256 leaves under key[0]=0x10, varying key[1] across all 256 values.
    // This creates two levels of branching:
    //   - A branch at nibble path [1,0] with children at all 16 nibbles (0-F)
    //   - Each child is itself a branch with 16 leaf children (e.g., [1,0,0], [1,0,1], …)
    //
    // When proofs are generated for all 256 keys, the proof includes branch nodes at
    // [1,0,0] through [1,0,F], each carrying their own masks (hash_mask with bits set
    // for their 16 hashed leaf children). These masks are registered in the trie's
    // internal `branch_node_masks` during `reveal_nodes`.
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u16..256 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i as u8;
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage.clone());

    // Initialize trie with masks and retain_updates=true.
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root once to cache branch hashes.
    let _ = trie.root();

    // Take and commit initial updates so the masks baseline is established.
    let initial_updates = trie.take_updates();
    trie.commit_updates(&initial_updates.updated_nodes, &initial_updates.removed_nodes);

    // Remove all 16 leaves from sub-branch [1,0,F] (key[1]=0xF0..0xFF).
    // This collapses the branch at [1,0,F] entirely — its masks were registered
    // during reveal, so it should appear in removed_nodes.
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0xF0u16..=0xFF {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i as u8;
        changeset.insert(key, U256::ZERO);
    }
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // Compute root to finalize changes.
    let _ = trie.root();

    let updates = trie.take_updates();

    // The parent branch at [1,0] should be updated: it lost child nibble F,
    // so state_mask and hash_mask should have bit F (0x8000) cleared.
    assert_eq!(updates.updated_nodes.len(), 1, "exactly one branch should be updated");
    let parent = updates.updated_nodes.get(&Nibbles::from_nibbles([0x1, 0x0])).unwrap();
    assert!(
        !parent.state_mask.is_bit_set(0xF),
        "parent branch should no longer have nibble F in state_mask"
    );
    assert!(
        !parent.hash_mask.is_bit_set(0xF),
        "parent branch should no longer have nibble F in hash_mask"
    );
    // Nibbles 0-E should still be present.
    for nibble in 0u8..0xF {
        assert!(parent.state_mask.is_bit_set(nibble), "nibble {nibble} should still be set");
    }

    // The sub-branch at [1,0,F] was fully removed — its masks were registered
    // during reveal, so it appears in removed_nodes.
    assert_eq!(updates.removed_nodes.len(), 1, "exactly one path should be removed");
    assert!(
        updates.removed_nodes.contains(&Nibbles::from_nibbles([0x1, 0x0, 0xF])),
        "removed_nodes should contain the collapsed sub-branch [1,0,F]"
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
