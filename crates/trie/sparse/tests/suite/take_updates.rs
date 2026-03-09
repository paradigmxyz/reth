use super::*;

pub(super) fn test_take_updates_returns_empty_when_not_tracking<T: SparseTrie + Default>() {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    let updates = trie.take_updates();
    assert!(updates.updated_nodes.is_empty(), "updated_nodes should be empty when not tracking");
    assert!(updates.removed_nodes.is_empty(), "removed_nodes should be empty when not tracking");
}

/// Consecutive takes are independent.
///
/// After `take_updates()`, subsequent updates should be tracked independently in a fresh
/// accumulator. updates1 reflects only the A mutation, updates2 reflects only the B mutation.
pub(super) fn test_take_updates_resets_after_take<T: SparseTrie + Default>() {
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Cache initial branch hashes.
    let _ = trie.root();

    // Round 1: add a new leaf A under the shared prefix, root, take.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    key_a.0[1] = 0xFF;
    let changeset_a: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::from(999))]);
    let mut leaf_updates_a = SuiteTestHarness::leaf_updates(&changeset_a);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_a);
    let _ = trie.root();
    let updates1 = trie.take_updates();

    assert!(
        !updates1.updated_nodes.is_empty() || !updates1.removed_nodes.is_empty(),
        "updates1 should be non-empty after adding leaf A",
    );

    // Immediately taking again (no new mutations) should yield empty updates,
    // proving the accumulator was reset by the first take.
    let updates_empty = trie.take_updates();
    assert!(
        updates_empty.updated_nodes.is_empty() && updates_empty.removed_nodes.is_empty(),
        "take_updates right after a take should be empty (accumulator was reset)",
    );

    // Round 2: add a new leaf B under the same prefix, root, take.
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x10;
    key_b.0[1] = 0xFE;
    let changeset_b: BTreeMap<B256, U256> = BTreeMap::from([(key_b, U256::from(888))]);
    let mut leaf_updates_b = SuiteTestHarness::leaf_updates(&changeset_b);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_b);
    let _ = trie.root();
    let updates2 = trie.take_updates();

    assert!(
        !updates2.updated_nodes.is_empty() || !updates2.removed_nodes.is_empty(),
        "updates2 should be non-empty after adding leaf B",
    );
}

/// `take_updates` contains both updated and removed nodes, mutually exclusive.
///
/// After mutations that cause branch node changes and deletions, `take_updates` should
/// contain both updated and removed nodes. The two sets must be mutually exclusive.
pub(super) fn test_take_updates_contains_updated_and_removed_nodes<T: SparseTrie + Default>() {
    // Two groups of 256 leaves each, creating two levels of branching per group.
    // Proofs for all keys register branch masks at [X,0,0] through [X,0,F] in the
    // trie's internal `branch_node_masks`, making removals trackable.
    //
    // Group 0x1: 256 leaves under key[0]=0x10, key[1]=0x00..0xFF
    //   → branch at [1,0] with sub-branches [1,0,0] through [1,0,F]
    // Group 0x2: 256 leaves under key[0]=0x20, key[1]=0x00..0xFF
    //   → branch at [2,0] with sub-branches [2,0,0] through [2,0,F]
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u16..256 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i as u8;
        storage.insert(key, U256::from(i as u64 + 1));
    }
    for i in 0u16..256 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20;
        key.0[1] = i as u8;
        storage.insert(key, U256::from(i as u64 + 1000));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Cache initial branch hashes.
    let _ = trie.root();

    // Commit initial updates to establish the masks baseline.
    let initial_updates = trie.take_updates();
    trie.commit_updates(&initial_updates.updated_nodes, &initial_updates.removed_nodes);

    // Remove all 256 leaves from group 0x2 (collapses that entire subtrie → removed_nodes)
    // and add a new leaf under group 0x1 (changes branch hashes → updated_nodes).
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u16..256 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20;
        key.0[1] = i as u8;
        changeset.insert(key, U256::ZERO);
    }
    // Add a new leaf in the 0x1 group to trigger updated_nodes for that subtrie.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    changeset.insert(new_key, U256::from(9999));

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root();
    let updates = trie.take_updates();

    // updated_nodes: the branch at [1,0] is updated because we modified a leaf
    // in group 0x1 (key[1]=0xFF → value 9999). Its masks remain fully populated
    // since all 256 leaves in group 0x1 still exist.
    assert_eq!(updates.updated_nodes.len(), 1, "exactly one branch should be updated");
    let parent = updates.updated_nodes.get(&Nibbles::from_nibbles([0x1, 0x0])).unwrap();
    for nibble in 0u8..16 {
        assert!(parent.state_mask.is_bit_set(nibble), "nibble {nibble} should still be set");
        assert!(parent.hash_mask.is_bit_set(nibble), "nibble {nibble} hash should still be set");
    }

    // removed_nodes: removing all 256 leaves from group 0x2 collapses the entire
    // subtrie. This removes:
    //   - The root-level branch node [] (previously had children at nibbles 1 and 2, now only has
    //     nibble 1 so it becomes an extension, not a stored branch)
    //   - The parent branch at [2,0]
    //   - All 16 sub-branches [2,0,0] through [2,0,F]
    // 18 total: root [], parent [2,0], and 16 sub-branches [2,0,0]..[2,0,F]
    assert_eq!(updates.removed_nodes.len(), 18, "expected 18 removed paths");
    assert!(
        updates.removed_nodes.contains(&Nibbles::default()),
        "root-level branch [] should be removed (collapsed to extension)"
    );
    assert!(
        updates.removed_nodes.contains(&Nibbles::from_nibbles([0x2, 0x0])),
        "parent branch [2,0] should be removed"
    );
    for nibble in 0u8..16 {
        assert!(
            updates.removed_nodes.contains(&Nibbles::from_nibbles([0x2, 0x0, nibble])),
            "sub-branch [2,0,{nibble:x}] should be removed"
        );
    }

    // The two sets must be mutually exclusive — no path in both.
    for path in &updates.removed_nodes {
        assert!(
            !updates.updated_nodes.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes"
        );
    }
}

/// Remove then re-insert at same path → sets are mutually exclusive.
///
/// When a branch collapses (leaf removal) and then a new branch is created at the same path
/// (leaf insertion), `take_updates` must not report the same path in both `updated_nodes` and
/// `removed_nodes`. The insertion must win.
pub(super) fn test_take_updates_no_duplicate_updated_and_removed_nodes<T: SparseTrie + Default>() {
    // 3 leaves sharing the first nibble → branch at nibble 0x0.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x00;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x01;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x02;

    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Cache initial hashes.
    let _ = trie.root();

    // Step 1: Remove key_c — with only 3 keys under the root branch, removing one causes
    // structural changes (branch may collapse or lose a child).
    let mut remove_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    remove_changeset.insert(key_c, U256::ZERO);
    let mut remove_updates = SuiteTestHarness::leaf_updates(&remove_changeset);
    harness.reveal_and_update(&mut trie, &mut remove_updates);

    // Step 2: Insert a new key at 0x03 — re-creates/modifies the branch structure at the
    // same path that was affected by the removal.
    let mut key_d = B256::ZERO;
    key_d.0[0] = 0x03;
    let mut insert_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    insert_changeset.insert(key_d, U256::from(4));
    let mut insert_updates = SuiteTestHarness::leaf_updates(&insert_changeset);
    harness.reveal_and_update(&mut trie, &mut insert_updates);

    // Finalize and take updates.
    let _ = trie.root();
    let updates = trie.take_updates();

    // The two sets must be mutually exclusive — no path in both.
    for path in &updates.removed_nodes {
        assert!(
            !updates.updated_nodes.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes"
        );
    }
}
