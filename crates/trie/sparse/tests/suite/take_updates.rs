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
    assert!(updates.updated_nodes.is_empty(), "updated_nodes should be empty when not tracking");
    assert!(updates.removed_nodes.is_empty(), "removed_nodes should be empty when not tracking");
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
/// Uses a 3-level branching structure so that intermediate branches are "real" DB nodes
/// (non-empty `BranchNodeMasks`). After removing one group entirely and modifying the
/// other, `take_updates` should report real branches in `removed_nodes` and modified
/// branches in `updated_nodes`, with the two sets mutually exclusive.
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
    // removed_nodes. The sub-sub-branches [2,0,H,L] have only leaf children
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
    let _ = trie.root();

    // Commit initial updates to establish the masks baseline.
    let initial_updates = trie.take_updates();
    trie.commit_updates(&initial_updates.updated_nodes, &initial_updates.removed_nodes);

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
    // Add a new leaf in the 0x1 group to trigger updated_nodes.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    new_key.0[2] = 0xFF;
    changeset.insert(new_key, U256::from(9999));

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root();
    let updates = trie.take_updates();

    // updated_nodes should contain at least the branch at [1,0] (modified group).
    assert!(
        updates.updated_nodes.contains_key(&Nibbles::from_nibbles([0x1, 0x0])),
        "branch [1,0] should be in updated_nodes after adding a leaf in group 0x1"
    );

    // removed_nodes should contain [2,0] — it was a "real" DB node (had branch
    // children → non-empty hash_mask) and was fully removed.
    assert!(
        updates.removed_nodes.contains(&Nibbles::from_nibbles([0x2, 0x0])),
        "[2,0] was a real DB node and should appear in removed_nodes"
    );

    // The 16 sub-branches [2,0,H] were also "real" DB nodes (each had branch
    // children at [2,0,H,L]) and should appear in removed_nodes.
    for nibble in 0u8..16 {
        let sub_path = Nibbles::from_nibbles([0x2, 0x0, nibble]);
        assert!(
            updates.removed_nodes.contains(&sub_path),
            "[2,0,{nibble:x}] was a real DB node and should appear in removed_nodes"
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

/// Multiple `root()` calls without intermediate `take_updates()` → sets are mutually exclusive.
///
/// When a branch is created inside the trie (giving it non-empty `hash_mask` →
/// `updated_nodes` entry) and then destroyed (clearing `hash_mask` → `removed_nodes`
/// entry) across two `root()` cycles without `take_updates()` in between, the
/// accumulated updates must cross-cancel so the same path does not appear in both.
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
    let _ = trie.root();

    // Changeset 1: insert key_new_a and key_new_b.
    let changeset1: BTreeMap<B256, U256> =
        [(key_new_a, val), (key_new_b, val)].into_iter().collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    let _ = trie.root();

    // Do NOT call take_updates() — updates accumulate across root() calls.

    // Changeset 2: remove key_new_a and key_new_b (undo changeset 1).
    let changeset2: BTreeMap<B256, U256> =
        [(key_new_a, U256::ZERO), (key_new_b, U256::ZERO)].into_iter().collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset2);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    let root_after_remove = trie.root();

    assert_eq!(
        harness.original_root(),
        root_after_remove,
        "root should match original after insert+remove round-trip"
    );

    let updates = trie.take_updates();

    for path in &updates.removed_nodes {
        assert!(
            !updates.updated_nodes.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes \
             (cross-cancellation bug)"
        );
    }
}

/// Remove then re-insert at same path → sets are mutually exclusive.
///
/// When a branch collapses (leaf removal) and then a new branch is created at the same path
/// (leaf insertion), `take_updates` must not report the same path in both `updated_nodes` and
/// `removed_nodes`. The insertion must win.
pub(super) fn test_take_updates_no_duplicate_updated_and_removed_nodes<T: SparseTrie>(
    new_trie: fn() -> T,
) {
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
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

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
