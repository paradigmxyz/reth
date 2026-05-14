use super::*;

/// After commit, `take_updates` reports only delta.
///
/// After calling `commit_updates` with taken updates, a subsequent mutation + `root()` +
/// `take_updates()` should only report the delta from the new baseline — it must NOT
/// re-report branch nodes from the first round.
pub(super) fn test_commit_updates_syncs_branch_masks<T: SparseTrie>(new_trie: fn() -> T) {
    // 5 leaves spread across different subtrie regions.
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
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    // Cache initial hashes.
    let _ = trie.root();

    // Round 1: Update leaf A, take updates, commit.
    let mut changeset1: BTreeMap<B256, U256> = BTreeMap::new();
    changeset1.insert(key_a, U256::from(100));
    let mut leaf_updates1 = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates1);

    let hash1 = trie.root();
    let updates1 = trie.take_updates();
    trie.commit_updates(&updates1.updated_nodes, &updates1.removed_nodes);

    // Round 2: Update leaf B, take updates.
    let mut changeset2: BTreeMap<B256, U256> = BTreeMap::new();
    changeset2.insert(key_b, U256::from(200));
    let mut leaf_updates2 = SuiteTestHarness::leaf_updates(&changeset2);
    harness.reveal_and_update(&mut trie, &mut leaf_updates2);

    let hash2 = trie.root();
    let updates2 = trie.take_updates();

    // hash2 should differ from hash1 (different leaf values).
    assert_ne!(hash1, hash2, "root should change after second update");

    // Verify hash2 matches the reference trie.
    let expected_storage: BTreeMap<B256, U256> = BTreeMap::from([
        (key_a, U256::from(100)),
        (key_b, U256::from(200)),
        (key_c, U256::from(3)),
        (key_d, U256::from(4)),
        (key_e, U256::from(5)),
    ]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(hash2, expected_harness.original_root(), "hash2 should match reference trie");

    // updates2 should NOT contain the same paths as updates1 — commit_updates
    // resets the baseline so only the delta from round 2 is reported.
    for path in updates1.updated_nodes.keys() {
        assert!(
            !updates2.updated_nodes.contains_key(path),
            "path {path:?} was re-reported in updates2 after commit — \
             commit_updates should have synced the baseline"
        );
    }
}

/// Committing empty updated/removed sets should not change trie behavior.
///
/// Build a trie, compute root, then commit empty updates. Root should be unchanged.
pub(super) fn test_commit_updates_empty_is_noop<T: SparseTrie>(new_trie: fn() -> T) {
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

    let hash1 = trie.root();
    assert_eq!(hash1, harness.original_root());

    trie.commit_updates(&HashMap::default(), &HashSet::default());

    let hash2 = trie.root();
    assert_eq!(hash1, hash2, "empty commit_updates should not change root");
}
