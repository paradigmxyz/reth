use super::*;

/// `size_hint` reflects leaf count changes.
///
/// Builds a 5-leaf trie, records `size_hint`, adds 2 leaves, records again,
/// removes 1 leaf, records again. Asserts s2 > s1 and s3 < s2 (monotonic
/// relative to leaf count changes).
pub(super) fn test_size_hint_reflects_leaf_count<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);
    let key4 = B256::with_last_byte(0x40);
    let key5 = B256::with_last_byte(0x50);
    let new_key1 = B256::with_last_byte(0x60);
    let new_key2 = B256::with_last_byte(0x70);

    let base_storage: BTreeMap<B256, U256> = BTreeMap::from([
        (key1, U256::from(1)),
        (key2, U256::from(2)),
        (key3, U256::from(3)),
        (key4, U256::from(4)),
        (key5, U256::from(5)),
    ]);

    let harness = SuiteTestHarness::new(base_storage);

    // Include new key targets so proofs cover them.
    let all_targets = vec![key1, key2, key3, key4, key5, new_key1, new_key2];
    let mut trie: T = harness.init_trie_with_targets(&all_targets, false);

    let s1 = trie.size_hint();

    // Add 2 new leaves.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([
        (new_key1, U256::from(6)),
        (new_key2, U256::from(7)),
    ]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after insert");

    let s2 = trie.size_hint();
    assert!(s2 > s1, "size_hint should increase after adding leaves: s2={s2} > s1={s1}");

    // Remove 1 leaf (set value to zero).
    let mut remove_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(new_key2, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut remove_updates);
    assert!(remove_updates.is_empty(), "leaf_updates should be drained after removal");

    let s3 = trie.size_hint();
    assert!(s3 < s2, "size_hint should decrease after removing a leaf: s3={s3} < s2={s2}");
}
