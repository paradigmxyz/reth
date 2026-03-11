use super::*;

pub(super) fn test_find_leaf_exists<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    let key2_nibbles = Nibbles::unpack(key2);

    // find_leaf with no expected value → Exists
    assert_eq!(
        trie.find_leaf(&key2_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::Exists,
        "find_leaf(existing_key, None) should return Exists"
    );

    // find_leaf with correct expected value → Exists
    let correct_value = encode_fixed_size(&U256::from(2)).to_vec();
    assert_eq!(
        trie.find_leaf(&key2_nibbles, Some(&correct_value)).expect("find_leaf should succeed"),
        LeafLookup::Exists,
        "find_leaf(existing_key, Some(correct_value)) should return Exists"
    );
}

pub(super) fn test_find_leaf_nonexistent<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    let nonexistent_key = B256::with_last_byte(0x99);
    let nonexistent_nibbles = Nibbles::unpack(nonexistent_key);

    assert_eq!(
        trie.find_leaf(&nonexistent_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "find_leaf(nonexistent_key, None) should return NonExistent"
    );
}

/// `find_leaf` on a path that traverses a blinded node returns
/// `Err(LeafLookupError::BlindedNode)`.
pub(super) fn test_find_leaf_blinded<T: SparseTrie + Default>() {
    // Use ≥16 keys per nibble group so branch children become hash nodes (>32 bytes RLP),
    // ensuring partial reveal leaves blinded subtries.
    let mut base_storage = BTreeMap::new();

    // 16 keys under first nibble 0x1
    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    // 16 keys under first nibble 0x2
    let mut group_b_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        group_b_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only group_a keys, leaving group_b's subtrie blinded.
    let trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    // Look up a key in group_b — should hit a blinded node.
    let blinded_key = group_b_keys[0];
    let blinded_nibbles = Nibbles::unpack(blinded_key);

    let result = trie.find_leaf(&blinded_nibbles, None);
    assert!(
        matches!(result, Err(LeafLookupError::BlindedNode { .. })),
        "find_leaf on a blinded path should return Err(BlindedNode), got: {result:?}"
    );
}

/// `find_leaf` with an expected value that doesn't match the actual leaf value
/// returns `Err(LeafLookupError::ValueMismatch)`.
pub(super) fn test_find_leaf_value_mismatch<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    let key2_nibbles = Nibbles::unpack(key2);
    let wrong_value = encode_fixed_size(&U256::from(999)).to_vec();

    let result = trie.find_leaf(&key2_nibbles, Some(&wrong_value));
    assert!(
        matches!(result, Err(LeafLookupError::ValueMismatch { .. })),
        "find_leaf with wrong expected value should return Err(ValueMismatch), got: {result:?}"
    );
}

/// `find_leaf` on a key whose nibble at a branch node
/// is not set in the branch's `state_mask` returns `NonExistent`.
///
/// Two leaves sharing prefix 0x12 create a branch with children at nibbles 3 and 5.
/// Searching for a key with nibble 7 at that branch position should return `NonExistent`.
pub(super) fn test_find_leaf_nonexistent_branch_divergence<T: SparseTrie + Default>() {
    // key1 nibbles: 1,2,3,4,0,0,...  →  B256 = 0x12340000...
    let mut key1 = B256::ZERO;
    key1.0[0] = 0x12;
    key1.0[1] = 0x34;

    // key2 nibbles: 1,2,5,6,0,0,...  →  B256 = 0x12560000...
    let mut key2 = B256::ZERO;
    key2.0[0] = 0x12;
    key2.0[1] = 0x56;

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    // search_path nibbles: 1,2,7,8,0,0,...  →  B256 = 0x12780000...
    // Nibble 7 is unset at the branch (only 3 and 5 are set).
    let mut search_key = B256::ZERO;
    search_key.0[0] = 0x12;
    search_key.0[1] = 0x78;
    let search_nibbles = Nibbles::unpack(search_key);

    assert_eq!(
        trie.find_leaf(&search_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "find_leaf on key with unset nibble at branch should return NonExistent"
    );
}

/// `find_leaf` on a key that diverges from an extension
/// node's key returns `NonExistent`.
///
/// A single leaf at nibbles [1,2,3,4,5,6,...] creates an extension root with key 0x12.
/// Searching for a key at nibbles [1,2,7,8,...] diverges from that extension.
pub(super) fn test_find_leaf_nonexistent_extension_divergence<T: SparseTrie + Default>() {
    // Single leaf: nibbles [1,2,3,4,5,6,0,0,...] → B256 = 0x12345600...
    let mut key1 = B256::ZERO;
    key1.0[0] = 0x12;
    key1.0[1] = 0x34;
    key1.0[2] = 0x56;

    let base_storage: BTreeMap<B256, U256> = once((key1, U256::from(1))).collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    // Search path diverges from the extension: nibbles [1,2,7,8,0,0,...] → B256 = 0x12780000...
    let mut search_key = B256::ZERO;
    search_key.0[0] = 0x12;
    search_key.0[1] = 0x78;
    let search_nibbles = Nibbles::unpack(search_key);

    assert_eq!(
        trie.find_leaf(&search_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "find_leaf on key diverging from extension should return NonExistent"
    );
}

/// `find_leaf` on a key that shares a prefix with an existing
/// leaf but has a longer path returns `NonExistent`.
///
/// A single leaf at nibbles [1,2,3,4,0,0,...] exists. Searching for a key at nibbles
/// [1,2,3,4,5,6,0,0,...] extends past the existing leaf — it should return `NonExistent`.
pub(super) fn test_find_leaf_nonexistent_leaf_divergence<T: SparseTrie + Default>() {
    // Existing leaf at short path: nibbles [1,2,3,4,0,0,...] → B256 = 0x12340000...
    let mut existing_key = B256::ZERO;
    existing_key.0[0] = 0x12;
    existing_key.0[1] = 0x34;

    let base_storage: BTreeMap<B256, U256> = once((existing_key, U256::from(1))).collect();

    let harness = SuiteTestHarness::new(base_storage);
    let trie: T = harness.init_trie_fully_revealed(false);

    // Search path extends past the existing leaf: nibbles [1,2,3,4,5,6,0,0,...]
    // → B256 = 0x12345600...
    let mut search_key = B256::ZERO;
    search_key.0[0] = 0x12;
    search_key.0[1] = 0x34;
    search_key.0[2] = 0x56;
    let search_nibbles = Nibbles::unpack(search_key);

    assert_eq!(
        trie.find_leaf(&search_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "find_leaf on key extending past existing leaf should return NonExistent"
    );
}
