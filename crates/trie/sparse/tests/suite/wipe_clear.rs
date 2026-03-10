use super::*;

/// Calling `wipe()` resets the trie so that
/// `root()` returns `EMPTY_ROOT_HASH`.
pub(super) fn test_wipe_resets_to_empty_root<T: SparseTrie + Default>() {
    let storage: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(0x10), U256::from(1)),
        (B256::with_last_byte(0x20), U256::from(2)),
        (B256::with_last_byte(0x30), U256::from(3)),
        (B256::with_last_byte(0x40), U256::from(4)),
        (B256::with_last_byte(0x50), U256::from(5)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Compute root to confirm the trie is populated.
    let root_before = trie.root();
    assert_eq!(root_before, harness.original_root());
    assert_ne!(root_before, EMPTY_ROOT_HASH);

    // Wipe and verify empty root.
    trie.wipe();
    let root_after = trie.root();
    assert_eq!(root_after, EMPTY_ROOT_HASH, "root must be EMPTY_ROOT_HASH after wipe");
}

/// `clear()` resets the trie to empty but preserves
/// update tracking mode. After clear, `root()` returns `EMPTY_ROOT_HASH` and
/// `take_updates()` returns empty (non-wiped) updates.
pub(super) fn test_clear_resets_trie_but_preserves_update_tracking<T: SparseTrie + Default>() {
    let storage: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(0x10), U256::from(1)),
        (B256::with_last_byte(0x20), U256::from(2)),
        (B256::with_last_byte(0x30), U256::from(3)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    // retain_updates = true so update tracking is active
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root to populate the trie fully.
    let root_before = trie.root();
    assert_eq!(root_before, harness.original_root());
    assert_ne!(root_before, EMPTY_ROOT_HASH);

    // Clear and verify empty root.
    trie.clear();
    let root_after = trie.root();
    assert_eq!(root_after, EMPTY_ROOT_HASH, "root must be EMPTY_ROOT_HASH after clear");

    // take_updates should return empty (non-wiped) updates since tracking is preserved.
    let updates = trie.take_updates();
    assert!(!updates.wiped, "clear should not produce wiped updates");
    assert!(updates.updated_nodes.is_empty(), "clear should produce empty updated_nodes");
    assert!(updates.removed_nodes.is_empty(), "clear should produce empty removed_nodes");
}

/// `wipe()` produces special "wiped" updates distinct
/// from normal empty updates. After wipe, `take_updates()` returns updates with
/// the wiped flag set and `root()` returns `EMPTY_ROOT_HASH`.
pub(super) fn test_wipe_produces_wiped_updates<T: SparseTrie + Default>() {
    let storage: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(0x10), U256::from(1)),
        (B256::with_last_byte(0x20), U256::from(2)),
        (B256::with_last_byte(0x30), U256::from(3)),
    ]);

    let harness = SuiteTestHarness::new(storage);
    // retain_updates = true so update tracking is active
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root to populate the trie fully.
    let root_before = trie.root();
    assert_eq!(root_before, harness.original_root());
    assert_ne!(root_before, EMPTY_ROOT_HASH);

    // Wipe the trie.
    trie.wipe();

    // Root should be EMPTY_ROOT_HASH after wipe.
    let root_after = trie.root();
    assert_eq!(root_after, EMPTY_ROOT_HASH, "root must be EMPTY_ROOT_HASH after wipe");

    // take_updates should return updates with the wiped flag set.
    let updates = trie.take_updates();
    assert!(updates.wiped, "wipe should produce wiped updates");
}

/// A cleared trie can be fully re-initialized and used
/// normally. After `clear()`, set a new root from a different dataset, reveal
/// nodes, insert a leaf, and verify `root()` matches the reference.
pub(super) fn test_clear_then_reuse_trie<T: SparseTrie + Default>() {
    // Phase 1: build a trie with 5 leaves and compute root.
    let storage_1: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(0x10), U256::from(1)),
        (B256::with_last_byte(0x20), U256::from(2)),
        (B256::with_last_byte(0x30), U256::from(3)),
        (B256::with_last_byte(0x40), U256::from(4)),
        (B256::with_last_byte(0x50), U256::from(5)),
    ]);
    let harness_1 = SuiteTestHarness::new(storage_1);
    let mut trie: T = harness_1.init_trie_fully_revealed(false);

    let root_1 = trie.root();
    assert_eq!(root_1, harness_1.original_root());
    assert_ne!(root_1, EMPTY_ROOT_HASH);

    // Phase 2: clear the trie and verify empty.
    trie.clear();
    assert_eq!(trie.root(), EMPTY_ROOT_HASH, "root must be EMPTY_ROOT_HASH after clear");

    // Phase 3: build a completely different dataset with 3 leaves.
    let storage_2: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(0xA0), U256::from(100)),
        (B256::with_last_byte(0xB0), U256::from(200)),
        (B256::with_last_byte(0xC0), U256::from(300)),
    ]);
    let mut harness_2 = SuiteTestHarness::new(storage_2.clone());

    // Re-initialize the trie with the new root and reveal all proof nodes.
    let root_node_2 = harness_2.root_node();
    trie.set_root(root_node_2.node, root_node_2.masks, false)
        .expect("set_root should succeed on cleared trie");

    let keys_2: Vec<B256> = storage_2.keys().copied().collect();
    let mut targets: Vec<ProofV2Target> = keys_2.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_nodes, _) = harness_2.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed on cleared trie");

    // Phase 4: insert a 4th leaf.
    let new_key = B256::with_last_byte(0xD0);
    let new_value = U256::from(400);
    let changeset = BTreeMap::from([(new_key, new_value)]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness_2.reveal_and_update(&mut trie, &mut leaf_updates);

    // Compute root and verify against reference.
    let root_2 = trie.root();

    // Update the reference harness with the 4th leaf.
    harness_2.apply_changeset(changeset);
    assert_eq!(root_2, harness_2.original_root(), "root after clear+reuse must match reference");
    assert_ne!(root_2, root_1, "new root must differ from pre-clear root");
    assert_ne!(root_2, EMPTY_ROOT_HASH, "new root must not be empty");
}
