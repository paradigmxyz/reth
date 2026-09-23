use super::*;

pub(super) fn test_take_updates_returns_empty_when_not_tracking<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    let updates = trie.take_updates();
    assert!(updates.is_empty(), "updates should be empty when not tracking");
}

/// Consecutive takes are independent.
///
/// After `take_updates()`, subsequent updates should be tracked independently in a fresh
/// accumulator. updates1 reflects only the A mutation, updates2 reflects only the B mutation.
pub(super) fn test_take_updates_resets_after_take<T: SparseTrie>(new_trie: fn() -> T) {
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    // Cache initial branch hashes.
    let _ = trie.root(epoch(0));

    // Round 1: add a new leaf A under the shared prefix, root, take.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    key_a.0[1] = 0xFF;
    let changeset_a: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::from(999))]);
    let mut leaf_updates_a = SuiteTestHarness::leaf_updates(&changeset_a);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_a);
    let _ = trie.root(epoch(0));
    let updates1 = trie.take_updates();

    assert!(!updates1.is_empty(), "updates1 should be non-empty after adding leaf A",);

    // Immediately taking again (no new mutations) should yield empty updates,
    // proving the accumulator was reset by the first take.
    let updates_empty = trie.take_updates();
    assert!(
        updates_empty.is_empty(),
        "take_updates right after a take should be empty (accumulator was reset)",
    );

    // Round 2: add a new leaf B under the same prefix, root, take.
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x10;
    key_b.0[1] = 0xFE;
    let changeset_b: BTreeMap<B256, U256> = BTreeMap::from([(key_b, U256::from(888))]);
    let mut leaf_updates_b = SuiteTestHarness::leaf_updates(&changeset_b);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_b);
    let _ = trie.root(epoch(0));
    let updates2 = trie.take_updates();

    assert!(!updates2.is_empty(), "updates2 should be non-empty after adding leaf B",);
}

/// `take_updates` contains both updated and removed nodes, mutually exclusive.
///
/// Uses a 3-level branching structure so that intermediate branches are "real" DB nodes
/// (non-empty `BranchNodeMasks`). After removing one group entirely and modifying the
/// other, `take_updates` should report deletions for persisted branches and insertions
/// for modified branches, sorted with one entry per path.
pub(super) fn test_take_updates_contains_updated_and_removed_nodes<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    // 3-level branching under two groups:
    //
    // Group 0x1 (survives, gets modified):
    //   key[0]=0x10 → branch at [1,0]
    //   key[1]=0x00..0xFF → sub-branches [1,0,H,L] for each nibble pair
    //   key[2]∈{0x00,0x10} → 2 leaf children per sub-branch
    //   Total: 512 leaves. [1,0,H] has branch children → "real" DB nodes.
    //
    // Group 0x2 (fully removed):
    //   Same structure under key[0]=0x20 → branch at [2,0]
    //   Total: 512 leaves. [2,0,H] has branch children → "real" DB nodes.
    //
    // After removing group 0x2 entirely, [2,0] and [2,0,H] should appear in
    // the deletions. The sub-sub-branches [2,0,H,L] have only leaf children
    // → empty masks → NOT in DB.
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    let mut val = 1u64;
    // Group 0x1: key[1]=0x00..0xFF × key[2]∈{0x00,0x10} = 512 leaves
    for key1 in 0u16..256 {
        for &k2 in &[0x00u8, 0x10] {
            let mut key = B256::ZERO;
            key.0[0] = 0x10;
            key.0[1] = key1 as u8;
            key.0[2] = k2;
            storage.insert(key, U256::from(val));
            val += 1;
        }
    }
    // Group 0x2: key[1]=0x00..0xFF × key[2]∈{0x00,0x10} = 512 leaves
    for key1 in 0u16..256 {
        for &k2 in &[0x00u8, 0x10] {
            let mut key = B256::ZERO;
            key.0[0] = 0x20;
            key.0[1] = key1 as u8;
            key.0[2] = k2;
            storage.insert(key, U256::from(val));
            val += 1;
        }
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    // Cache initial branch hashes.
    let _ = trie.root(epoch(0));

    // Drain initial updates before the mutation under test.
    let _ = trie.take_updates();

    // Remove all 512 leaves from group 0x2 and add a new leaf in group 0x1.
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    for key1 in 0u16..256 {
        for &k2 in &[0x00u8, 0x10] {
            let mut key = B256::ZERO;
            key.0[0] = 0x20;
            key.0[1] = key1 as u8;
            key.0[2] = k2;
            changeset.insert(key, U256::ZERO);
        }
    }
    // Add a new leaf in the 0x1 group to trigger insertions.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    new_key.0[2] = 0xFF;
    changeset.insert(new_key, U256::from(9999));

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root(epoch(0));
    let updates = trie.take_updates();

    // Insertions should contain at least the branch at [1,0] (modified group).
    assert!(
        updates
            .iter()
            .any(|(path, node)| *path == Nibbles::from_nibbles([0x1, 0x0]) && node.is_some()),
        "branch [1,0] should have an insertion after adding a leaf in group 0x1"
    );

    // Deletions should contain [2,0] — it was a "real" DB node (had branch
    // children → non-empty hash_mask) and was fully removed.
    assert!(
        updates.contains(&(Nibbles::from_nibbles([0x2, 0x0]), None)),
        "[2,0] was a real DB node and should have a deletion"
    );

    // The 16 sub-branches [2,0,H] were also "real" DB nodes (each had branch
    // children at [2,0,H,L]) and should have a deletion.
    for nibble in 0u8..16 {
        let sub_path = Nibbles::from_nibbles([0x2, 0x0, nibble]);
        assert!(
            updates.contains(&(sub_path, None)),
            "[2,0,{nibble:x}] was a real DB node and should have a deletion"
        );
    }

    assert!(updates.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

/// A later deletion overrides an insertion across multiple `root()` calls.
///
/// When a branch is created inside the trie (giving it non-empty `hash_mask` →
/// insertion) and then destroyed (clearing `hash_mask` → deletion) across two
/// `root()` cycles without `take_updates()` in between, only the deletion must remain.
///
/// Key structure (nibble paths):
/// - Initial: `0xAA12...` `[A,A,1,2,...]`, `0xAA20...` `[A,A,2,0,...]`, `0xBB00...` `[B,B,...]`
/// - Changeset 1 inserts `0xAA10_00...` and `0xAA10_10...`, which share `[A,A,1,0]` and diverge at
///   nibble 4, creating branch `[A,A,1,0]` (no short key) as a child of branch `[A,A,1]`. This
///   gives `[A,A,1]` non-empty `hash_mask`.
/// - Changeset 2 removes both, collapsing `[A,A,1]` back to a leaf with empty masks.
pub(super) fn test_take_updates_cross_cancellation_across_root_calls<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let val = U256::from(1u64);

    let mut key_existing = B256::ZERO;
    key_existing.0[0] = 0xAA;
    key_existing.0[1] = 0x12;

    let mut key_b = B256::ZERO;
    key_b.0[0] = 0xAA;
    key_b.0[1] = 0x20;

    let mut key_other = B256::ZERO;
    key_other.0[0] = 0xBB;

    let mut key_new_a = B256::ZERO;
    key_new_a.0[0] = 0xAA;
    key_new_a.0[1] = 0x10;

    let mut key_new_b = B256::ZERO;
    key_new_b.0[0] = 0xAA;
    key_new_b.0[1] = 0x10;
    key_new_b.0[2] = 0x10;

    let initial: BTreeMap<B256, U256> =
        [(key_existing, val), (key_b, val), (key_other, val)].into_iter().collect();

    let harness = SuiteTestHarness::new(initial);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    // Cache initial branch hashes.
    let _ = trie.root(epoch(0));

    // Changeset 1: insert key_new_a and key_new_b.
    let changeset1: BTreeMap<B256, U256> =
        [(key_new_a, val), (key_new_b, val)].into_iter().collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    let _ = trie.root(epoch(0));

    // Do NOT call take_updates() — updates accumulate across root() calls.

    // Changeset 2: remove key_new_a and key_new_b (undo changeset 1).
    let changeset2: BTreeMap<B256, U256> =
        [(key_new_a, U256::ZERO), (key_new_b, U256::ZERO)].into_iter().collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset2);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    let root_after_remove = trie.root(epoch(0));

    assert_eq!(
        harness.original_root(),
        root_after_remove,
        "root should match original after insert+remove round-trip"
    );

    let updates = trie.take_updates();

    assert!(updates.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert!(updates.contains(&(Nibbles::from_nibbles([0xa, 0xa, 1]), None)));
}

/// Reinsertion overrides a deletion, and later hashes override earlier node values.
pub(super) fn test_take_updates_no_duplicate_updated_and_removed_nodes<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let key = |bytes: &[u8]| {
        let mut key = B256::ZERO;
        key.0[..bytes.len()].copy_from_slice(bytes);
        key
    };
    let key_a = key(&[0xaa, 0x10, 0x00]);
    let key_b = key(&[0xaa, 0x10, 0x10]);
    let storage = BTreeMap::from([
        (key(&[0xaa, 0x12]), U256::from(1)),
        (key(&[0xaa, 0x20]), U256::from(2)),
        (key(&[0xbb]), U256::from(3)),
        (key_a, U256::from(4)),
        (key_b, U256::from(5)),
    ]);
    let harness = SuiteTestHarness::new(storage);
    let branch_path = Nibbles::from_nibbles([0xa, 0xa, 1]);

    for hash_after_removal in [false, true] {
        let mut trie = harness.init_trie_fully_revealed(true, new_trie);
        trie.root(epoch(0));
        trie.take_updates();

        let removals = BTreeMap::from([(key_a, U256::ZERO), (key_b, U256::ZERO)]);
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&removals));
        if hash_after_removal {
            trie.root(epoch(1));
        }

        let reinsertions = BTreeMap::from([(key_a, U256::from(6)), (key_b, U256::from(7))]);
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&reinsertions));
        trie.root(epoch(2));

        let changes = BTreeMap::from([(key_a, U256::from(8)), (key_b, U256::from(9))]);
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&changes));
        let root = trie.root(epoch(3));
        let updates = trie.take_updates();

        let (expected_root, expected_updates) = harness.get_root_with_updates(&changes);
        assert_eq!(root, expected_root);
        assert!(updates.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let expected_branch = expected_updates.storage_nodes.get(&branch_path).unwrap();
        assert_eq!(
            updates.iter().find(|(path, _)| *path == branch_path),
            Some(&(branch_path, Some(expected_branch.clone()))),
        );
        assert!(trie.take_updates().is_empty());
    }
}
