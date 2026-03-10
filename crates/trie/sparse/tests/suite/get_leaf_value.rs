use super::*;

/// After inserting or updating a leaf via `update_leaves`, `get_leaf_value`
/// should return the new value.
pub(super) fn test_get_leaf_value_after_update<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Insert a new leaf (key4) with value 42.
    let key4 = B256::with_last_byte(0x40);
    let new_value = U256::from(42);
    let new_value_rlp = encode_fixed_size(&new_value).to_vec();
    let mut leaf_updates: B256Map<LeafUpdate> =
        once((key4, LeafUpdate::Changed(new_value_rlp.clone()))).collect();
    trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");

    assert_eq!(
        trie.get_leaf_value(&Nibbles::unpack(key4)),
        Some(&new_value_rlp),
        "get_leaf_value should return the inserted value"
    );

    // Modify an existing leaf (key2) to value 222.
    let updated_value = U256::from(222);
    let updated_value_rlp = encode_fixed_size(&updated_value).to_vec();
    let mut leaf_updates: B256Map<LeafUpdate> =
        once((key2, LeafUpdate::Changed(updated_value_rlp.clone()))).collect();
    trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");

    assert_eq!(
        trie.get_leaf_value(&Nibbles::unpack(key2)),
        Some(&updated_value_rlp),
        "get_leaf_value should return the modified value"
    );
}

/// After removing a leaf via `update_leaves`, `get_leaf_value` should return `None`.
pub(super) fn test_get_leaf_value_after_removal<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    let key2_nibbles = Nibbles::unpack(key2);
    let expected_value_rlp = encode_fixed_size(&U256::from(2)).to_vec();
    assert_eq!(
        trie.get_leaf_value(&key2_nibbles),
        Some(&expected_value_rlp),
        "get_leaf_value should return Some before removal"
    );

    // Remove key2 by setting its value to U256::ZERO (produces LeafUpdate::Changed(vec![])).
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key2, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    assert_eq!(
        trie.get_leaf_value(&key2_nibbles),
        None,
        "get_leaf_value should return None after removal"
    );
}
