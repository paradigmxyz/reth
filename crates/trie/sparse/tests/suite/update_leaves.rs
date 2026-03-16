use super::*;

/// Insert a new leaf via `update_leaves` and verify root.
///
/// Starting from a 3-leaf trie, inserting a 4th key via `update_leaves` should produce
/// a root hash matching a reference trie containing all 4 leaves.
pub(super) fn test_update_leaves_insert_new_leaf<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);
    let new_key = B256::with_last_byte(0x40);

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))]);

    let harness = SuiteTestHarness::new(base_storage);

    // Initialize trie with all 3 existing keys revealed, plus the new key target.
    let all_targets = vec![key1, key2, key3, new_key];
    let mut trie: T = harness.init_trie_with_targets(&all_targets, true);

    // Insert the new leaf.
    let new_value = U256::from(4);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(new_key, new_value)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // The update map should be drained on success.
    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after successful insert");

    let root = trie.root();

    // Compute expected root with all 4 leaves.
    let expected_storage = BTreeMap::from([
        (key1, U256::from(1)),
        (key2, U256::from(2)),
        (key3, U256::from(3)),
        (new_key, new_value),
    ]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie with 4 leaves"
    );
}

/// Modify an existing leaf's value and verify root.
///
/// Starting from a 3-leaf trie, changing one leaf's value via `update_leaves` should
/// produce a root hash matching a reference trie with the updated value.
pub(super) fn test_update_leaves_modify_existing_leaf<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Modify an existing leaf with a new value.
    let new_value = U256::from(999);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key2, new_value)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after successful modify");

    let root = trie.root();

    // Compute expected root with the modified value.
    let expected_storage =
        BTreeMap::from([(key1, U256::from(1)), (key2, new_value), (key3, U256::from(3))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie with modified leaf value"
    );
}

/// Insert a single leaf into an empty trie.
///
/// Calling `update_leaves` with one key on a default (empty) trie should produce a root
/// hash matching a reference trie with that single leaf.
pub(super) fn test_insert_single_leaf_into_empty_trie<T: SparseTrie + Default>() {
    let key = B256::with_last_byte(42);
    let value = U256::from(1);

    let mut trie = T::default();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key, value)]));

    // Empty trie has no blinded nodes, so update_leaves should succeed in one call.
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected on empty trie");
    })
    .expect("update_leaves should succeed");

    assert!(leaf_updates.is_empty(), "leaf_updates should be drained");

    let root = trie.root();

    let expected_harness = SuiteTestHarness::new(BTreeMap::from([(key, value)]));
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference single-leaf trie"
    );
}

/// Insert 256 leaves with varied prefix patterns into an empty trie.
///
/// All 256 keys are inserted in a single `update_leaves` call. The root must match
/// a reference trie and `take_updates()` must return non-empty results.
pub(super) fn test_insert_multiple_leaves_into_empty_trie<T: SparseTrie + Default>() {
    // Build 256 keys with alternating prefix patterns (matching original test).
    let storage: BTreeMap<B256, U256> = (0..=255u8)
        .map(|b| {
            let key = if b % 2 == 0 { B256::repeat_byte(b) } else { B256::with_last_byte(b) };
            (key, U256::from(b as u64 + 1))
        })
        .collect();

    let expected_harness = SuiteTestHarness::new(storage.clone());

    let mut trie = T::default();
    trie.set_updates(true);

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&storage);

    // Empty trie has no blinded nodes, so one call should drain all updates.
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected on empty trie");
    })
    .expect("update_leaves should succeed");

    assert!(leaf_updates.is_empty(), "leaf_updates should be drained");

    let root = trie.root();
    assert_eq!(root, expected_harness.original_root(), "root should match reference 256-leaf trie");

    let updates = trie.take_updates();
    assert!(
        !updates.updated_nodes.is_empty(),
        "take_updates should contain updated nodes after 256 insertions"
    );
}

/// Overwrite all values and verify both root states.
///
/// Insert 256 keys with old values, compute root (hash1). Then update all 256 keys with
/// new values, compute root (hash2). Both must match their respective reference tries,
/// and hash1 ≠ hash2.
pub(super) fn test_update_all_leaves_with_new_values<T: SparseTrie + Default>() {
    // Build 256 keys with alternating prefix patterns.
    let keys: Vec<B256> = (0..=255u8)
        .map(|b| if b % 2 == 0 { B256::repeat_byte(b) } else { B256::with_last_byte(b) })
        .collect();

    let old_storage: BTreeMap<B256, U256> = keys.iter().map(|&k| (k, U256::from(1))).collect();

    let new_storage: BTreeMap<B256, U256> = keys.iter().map(|&k| (k, U256::from(999))).collect();

    let expected_old = SuiteTestHarness::new(old_storage.clone());
    let expected_new = SuiteTestHarness::new(new_storage.clone());

    let mut trie = T::default();
    trie.set_updates(true);

    // Insert all 256 keys with old values.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&old_storage);
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected on empty trie");
    })
    .expect("update_leaves should succeed");

    let hash1 = trie.root();
    assert_eq!(hash1, expected_old.original_root(), "hash1 should match reference with old values");

    // Update all 256 keys with new values.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&new_storage);
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected — all paths already revealed");
    })
    .expect("update_leaves should succeed");

    let hash2 = trie.root();
    assert_eq!(hash2, expected_new.original_root(), "hash2 should match reference with new values");
    assert_ne!(hash1, hash2, "roots should differ after updating all values");
}

/// Two leaves at adjacent keys produce correct root.
///
/// Insert key `0x50..` then key `0x51..` (adjacent first-byte keys that share first nibble `5`),
/// computing root after each. The final root must match the reference trie with both keys.
/// `take_updates()` should return empty since no branch masks were set.
pub(super) fn test_two_leaves_at_adjacent_keys_root_correctness<T: SparseTrie + Default>() {
    let mut key_50 = B256::ZERO;
    key_50.0[0] = 0x50;
    let mut key_51 = B256::ZERO;
    key_51.0[0] = 0x51;
    let value = U256::from(1);

    let mut trie = T::default();
    trie.set_updates(true);

    // Insert first leaf and compute root.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_50, value)]));
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected on empty trie");
    })
    .expect("update_leaves should succeed");
    trie.root();

    // Insert second leaf and compute root.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_51, value)]));
    trie.update_leaves(&mut leaf_updates, |_, _| {
        panic!("no proof callback expected — all paths already revealed");
    })
    .expect("update_leaves should succeed");
    let root = trie.root();

    let expected = SuiteTestHarness::new(BTreeMap::from([(key_50, value), (key_51, value)]));
    assert_eq!(root, expected.original_root(), "root should match reference two-leaf trie");

    let updates = trie.take_updates();
    assert!(
        updates.updated_nodes.is_empty() && updates.removed_nodes.is_empty(),
        "take_updates should be empty — no branch masks were set"
    );
}

/// Remove a leaf via `LeafUpdate::Changed(vec![])` and verify root.
///
/// Starting from a 3-leaf trie, removing one key should produce a root hash
/// matching a reference trie containing only the remaining 2 leaves.
pub(super) fn test_update_leaves_remove_leaf<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Remove key2 by setting its value to U256::ZERO (produces LeafUpdate::Changed(vec![])).
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key2, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after successful removal");

    let root = trie.root();

    // Expected: reference trie with only key1 and key3.
    let expected_storage = BTreeMap::from([(key1, U256::from(1)), (key3, U256::from(3))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie with removed leaf"
    );
}

/// Removing a leaf that causes a branch to collapse into an
/// extension. Three leaves sharing prefix `0x5` create a branch at nibble 5; removing one
/// child should collapse the structure. The root hash must match a reference trie with the
/// remaining two leaves.
pub(super) fn test_remove_leaf_branch_collapses_to_extension<T: SparseTrie + Default>() {
    // Keys sharing prefix 0x5: two share 0x50 (children at 0x502..) and one at 0x53.
    // This creates a branch at nibble 5 with children at nibbles 0 and 3.
    let mut key_50231 = B256::ZERO;
    key_50231.0[0] = 0x50;
    key_50231.0[1] = 0x23;
    key_50231.0[2] = 0x10;

    let mut key_50233 = B256::ZERO;
    key_50233.0[0] = 0x50;
    key_50233.0[1] = 0x23;
    key_50233.0[2] = 0x30;

    let mut key_537 = B256::ZERO;
    key_537.0[0] = 0x53;
    key_537.0[1] = 0x70;

    let base_storage: BTreeMap<B256, U256> = BTreeMap::from([
        (key_50231, U256::from(1)),
        (key_50233, U256::from(2)),
        (key_537, U256::from(3)),
    ]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Remove the leaf at key_537 — this collapses the branch at 0x5.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_537, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root = trie.root();

    // Expected: reference trie with only the two remaining leaves.
    let expected_storage = BTreeMap::from([(key_50231, U256::from(1)), (key_50233, U256::from(2))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie after branch-to-extension collapse"
    );
}

/// Removing one of two leaves from a branch should collapse the
/// branch into a leaf. Update tracking should report the root branch as removed and NOT as
/// updated.
pub(super) fn test_remove_leaf_branch_collapses_to_leaf<T: SparseTrie + Default>() {
    // Two leaves with different first nibbles → branch root.
    let key_a = B256::with_last_byte(0x10); // first nibble = 1
    let key_b = B256::with_last_byte(0x20); // first nibble = 2

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root to cache hashes, take and commit updates to establish baseline masks.
    let _ = trie.root();
    let updates = trie.take_updates();
    trie.commit_updates(&updates.updated_nodes, &updates.removed_nodes);

    // Remove key_a → branch collapses to a single leaf (key_b).
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_a, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root = trie.root();

    // Expected: reference trie with only key_b.
    let expected_storage = BTreeMap::from([(key_b, U256::from(200))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie after branch-to-leaf collapse"
    );

    let updates = trie.take_updates();

    // The root branch path is Nibbles::default() (empty path).
    let root_path = Nibbles::default();

    // The collapsed root branch should NOT appear in updated_nodes.
    assert!(
        !updates.updated_nodes.contains_key(&root_path),
        "root branch path should NOT be in updated_nodes after collapse"
    );
}

/// Removing the only leaf in a trie should produce
/// `EMPTY_ROOT_HASH`.
pub(super) fn test_remove_last_leaf_produces_empty_root<T: SparseTrie + Default>() {
    let key = B256::with_last_byte(0x12);
    let base_storage: BTreeMap<B256, U256> = BTreeMap::from([(key, U256::from(1))]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Remove the only leaf.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root = trie.root();
    assert_eq!(root, EMPTY_ROOT_HASH, "removing the only leaf should produce EMPTY_ROOT_HASH");
}

/// Build 6 leaves then remove one-by-one, verifying root at each
/// step against a reference trie. Final removal produces `EMPTY_ROOT_HASH`.
pub(super) fn test_insert_then_remove_sequence<T: SparseTrie + Default>() {
    // Helper: build a B256 key from a nibble prefix, zero-padded.
    let key_from_nibbles = |nibbles: &[u8]| -> B256 {
        let mut bytes = [0u8; 32];
        for (i, chunk) in nibbles.chunks(2).enumerate() {
            let hi = chunk[0];
            let lo = if chunk.len() > 1 { chunk[1] } else { 0 };
            bytes[i] = (hi << 4) | lo;
        }
        B256::from(bytes)
    };

    // The 6 keys from the original test (nibble prefixes 50231, 50233, 52013, 53102, 53302,
    // 53320).
    let k1 = key_from_nibbles(&[0x5, 0x0, 0x2, 0x3, 0x1]);
    let k2 = key_from_nibbles(&[0x5, 0x0, 0x2, 0x3, 0x3]);
    let k3 = key_from_nibbles(&[0x5, 0x2, 0x0, 0x1, 0x3]);
    let k4 = key_from_nibbles(&[0x5, 0x3, 0x1, 0x0, 0x2]);
    let k5 = key_from_nibbles(&[0x5, 0x3, 0x3, 0x0, 0x2]);
    let k6 = key_from_nibbles(&[0x5, 0x3, 0x3, 0x2, 0x0]);

    let val = U256::from(1);
    let all_keys = [k1, k2, k3, k4, k5, k6];

    // Insert all 6 leaves into empty trie.
    let base_storage: BTreeMap<B256, U256> = all_keys.iter().map(|&k| (k, val)).collect();

    let mut harness = SuiteTestHarness::new(base_storage.clone());
    let mut trie = T::default();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&base_storage);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root_after_insert = trie.root();
    assert_eq!(root_after_insert, harness.original_root(), "root after inserting all 6 leaves");

    // Remove leaves one at a time in the same order as the original test: k3, k1, k4, k6, k2, k5.
    let removal_order = [k3, k1, k4, k6, k2, k5];
    for (i, key) in removal_order.iter().enumerate() {
        let changeset = BTreeMap::from([(*key, U256::ZERO)]);
        harness.apply_changeset(changeset.clone());
        let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
        harness.reveal_and_update(&mut trie, &mut leaf_updates);

        let root = trie.root();
        assert_eq!(
            root,
            harness.original_root(),
            "root mismatch after removal step {} (removed key {:?})",
            i + 1,
            key
        );
    }

    // After all removals, trie should be empty.
    assert_eq!(trie.root(), EMPTY_ROOT_HASH, "final root should be EMPTY_ROOT_HASH");
}

/// Removing a nonexistent key should not invalidate cached hashes.
///
/// After computing `root()` (which caches hashes on all nodes), attempting to remove a key
/// that doesn't exist should leave the cache intact so the next `root()` call returns the
/// same hash without recomputation.
pub(super) fn test_remove_nonexistent_leaf_preserves_hashes<T: SparseTrie + Default>() {
    let key_a = B256::with_last_byte(0x10);
    let key_b = B256::with_last_byte(0x20);
    let key_c = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Compute root to cache hashes on all nodes.
    let root_before = trie.root();
    assert_eq!(root_before, harness.original_root());

    // Try to remove a key that doesn't exist in the trie.
    let nonexistent_key = B256::with_last_byte(0x50);
    let mut leaf_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(nonexistent_key, U256::ZERO)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // Root should be identical — cached hashes should be preserved.
    let root_after = trie.root();
    assert_eq!(
        root_before, root_after,
        "removing a nonexistent leaf should preserve cached hashes and return the same root"
    );
}

/// When `update_leaves` encounters a blinded node (insufficient
/// proof data), it should invoke the `proof_required_fn` callback with the correct target key
/// and minimum depth, and leave the key in the updates map for retry.
pub(super) fn test_update_leaves_blinded_node_requests_proof<T: SparseTrie + Default>() {
    // Use enough keys under two different first nibbles so that branch children become
    // hash nodes (>32 bytes RLP). This ensures partial reveal leaves blinded subtries.
    let mut base_storage = BTreeMap::new();

    // 16 keys under first nibble 0x1 (slots 0x10..0x1F)
    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    // 16 keys under first nibble 0x2 (slots 0x20..0x2F)
    let mut group_b_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        group_b_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only group_a keys, leaving group_b's subtrie blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    // Try to modify a key in group_b's blinded subtrie.
    let target_key = group_b_keys[0];
    let mut leaf_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(target_key, U256::from(999))]));

    // First update_leaves call: should invoke callback for the blinded node.
    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // The callback should have been invoked.
    assert!(!targets.is_empty(), "callback should be invoked for a blinded node");

    // The key should remain in the updates map (not drained).
    assert!(
        !leaf_updates.is_empty(),
        "key should remain in updates map when blinded node is encountered"
    );
}

pub(super) fn test_update_leaves_retry_after_reveal<T: SparseTrie + Default>() {
    // Same setup as blinded_node_requests_proof: two groups of 16 keys each under
    // different first nibbles, so branch children become hash nodes.
    let mut base_storage = BTreeMap::new();

    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    let mut group_b_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        group_b_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage.clone());

    // Reveal only group_a keys, leaving group_b's subtrie blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    // Modify a key in group_b's blinded subtrie.
    let target_key = group_b_keys[0];
    let new_value = U256::from(999);
    let mut leaf_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(target_key, new_value)]));

    // First update_leaves: callback fires, key stays in map.
    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");
    assert!(!targets.is_empty(), "callback should fire for blinded node");
    assert!(!leaf_updates.is_empty(), "key should remain in map after blinded hit");

    // Reveal the proof for the requested targets.
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");

    // Second update_leaves: now the path is revealed, key should be drained.
    let mut targets2: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets2.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed on retry");
    assert!(targets2.is_empty(), "no callback should fire after reveal");
    assert!(leaf_updates.is_empty(), "key should be drained after successful retry");

    // Root should match the expected trie with the modified value.
    let mut expected_storage = base_storage;
    expected_storage.insert(target_key, new_value);
    let expected_harness = SuiteTestHarness::new(expected_storage);

    let root = trie.root();
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match expected trie after retry"
    );
}

pub(super) fn test_remove_leaf_blinded_sibling_requires_reveal<T: SparseTrie + Default>() {
    // Build a branch with two children: one revealed leaf at nibble 0x1, and a blinded
    // subtrie at nibble 0x2 (16 keys so it becomes a hash node > 32 bytes).
    let mut base_storage = BTreeMap::new();

    // Single key under first nibble 0x1.
    let revealed_key = {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key
    };
    base_storage.insert(revealed_key, U256::from(1));

    // 16 keys under first nibble 0x2 (slots 0x20..0x2F) — enough to produce a hash node.
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage.clone());

    // Reveal only the single key at nibble 0x1, leaving nibble 0x2's subtrie blinded.
    let mut trie: T = harness.init_trie_with_targets(&[revealed_key], false);

    // Try to remove the revealed leaf. Branch collapse requires the blinded sibling.
    let mut leaf_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(revealed_key, U256::ZERO)]));

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");
    assert!(!targets.is_empty(), "callback should fire for blinded sibling");
    assert!(!leaf_updates.is_empty(), "key should remain in map after blinded hit");

    // Reveal the blinded sibling subtrie.
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");

    // Retry removal — now the sibling is revealed, branch can collapse.
    trie.update_leaves(&mut leaf_updates, |_, _| {})
        .expect("update_leaves should succeed on retry");
    assert!(leaf_updates.is_empty(), "key should be drained after successful removal");

    // Root should match reference trie with only group_b keys.
    let mut expected_storage = base_storage;
    expected_storage.remove(&revealed_key);
    let expected_harness = SuiteTestHarness::new(expected_storage);

    let root = trie.root();
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match trie without the removed leaf"
    );
}

/// Atomic rollback — node structure and `prefix_set` unchanged after
/// removal of a revealed leaf when the sibling is blinded.
///
/// Same scenario as the value-preservation test, but additionally checks that
/// `size_hint` (node count) is unchanged and the update remains in the map.
pub(super) fn test_update_leaves_removal_branch_collapse_blinded_sibling<
    T: SparseTrie + Default,
>() {
    // Branch: nibble 0x1 = one revealed leaf, nibble 0x2 = 16 blinded keys (hash node).
    let mut base_storage = BTreeMap::new();

    let revealed_key = {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key
    };
    base_storage.insert(revealed_key, U256::from(1));

    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only the leaf at nibble 0x1, leaving nibble 0x2 blinded.
    let mut trie: T = harness.init_trie_with_targets(&[revealed_key], false);

    // Snapshot state before the removal attempt.
    let revealed_path = Nibbles::unpack(revealed_key);
    let original_value = trie.get_leaf_value(&revealed_path).cloned();
    assert!(original_value.is_some(), "revealed leaf should exist");
    let node_count_before = trie.size_hint();

    // Attempt removal — branch collapse needs the blinded sibling, so it fails atomically.
    let mut leaf_updates =
        SuiteTestHarness::leaf_updates(&BTreeMap::from([(revealed_key, U256::ZERO)]));

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // Callback should have fired for the blinded sibling path.
    assert!(!targets.is_empty(), "callback should fire for blinded sibling");

    // Update should remain in the map (not drained).
    assert!(!leaf_updates.is_empty(), "update should remain in map after blinded hit");

    // Node count should be unchanged (no structural modification).
    assert_eq!(trie.size_hint(), node_count_before, "node count should be unchanged");

    // Leaf value should be preserved (atomic rollback).
    let value_after = trie.get_leaf_value(&revealed_path);
    assert_eq!(
        value_after,
        original_value.as_ref(),
        "leaf value should be preserved after failed removal"
    );
}

/// Subtrie collapse with blinded sibling triggers callback.
///
/// When removals in a subtrie would empty it and collapse the parent branch onto
/// a blinded sibling, `update_leaves` should detect this and request a proof for
/// the blinded sibling via the callback, deferring the updates.
pub(super) fn test_update_leaves_subtrie_collapse_requests_proof<T: SparseTrie + Default>() {
    // Build a branch with two children:
    //   nibble 0x1 → a subtrie with 2 revealed leaves
    //   nibble 0x2 → 16 blinded keys (hash node > 32 bytes)
    let mut base_storage = BTreeMap::new();

    // Two leaves under first nibble 0x1 (will be revealed and then removed).
    let subtrie_key_a = {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key
    };
    let subtrie_key_b = {
        let mut key = B256::ZERO;
        key.0[0] = 0x11;
        key
    };
    base_storage.insert(subtrie_key_a, U256::from(1));
    base_storage.insert(subtrie_key_b, U256::from(2));

    // 16 keys under first nibble 0x2 — enough to produce a hash node.
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only the two subtrie keys at nibble 0x1, leaving nibble 0x2 blinded.
    let mut trie: T = harness.init_trie_with_targets(&[subtrie_key_a, subtrie_key_b], false);

    // Remove both leaves in the subtrie — this would empty the subtrie and
    // require collapsing the parent branch onto the blinded sibling at nibble 0x2.
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([
        (subtrie_key_a, U256::ZERO),
        (subtrie_key_b, U256::ZERO),
    ]));

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // Callback should have fired for the blinded sibling.
    assert!(
        !targets.is_empty(),
        "callback should fire for blinded sibling during subtrie collapse"
    );
    // At least one removal key should remain in the map for retry.
    assert!(!leaf_updates.is_empty(), "removal keys should remain in map after blinded hit");
}

/// Multiple keys hitting the same blinded node each trigger a callback.
///
/// When multiple keys in the update map all route through the same blinded node,
/// the callback should be invoked once per key (not deduplicated).
pub(super) fn test_update_leaves_multiple_keys_same_blinded_node<T: SparseTrie + Default>() {
    // Branch: nibble 0x1 = 16 revealed keys (hash node), nibble 0x2 = 16 blinded keys.
    let mut base_storage = BTreeMap::new();

    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only group_a, leaving nibble 0x2 blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    // Submit 3 keys that all start with nibble 0x2 — they all hit the same blinded node.
    let blinded_keys: BTreeMap<B256, U256> = (0u8..3)
        .map(|i| {
            let mut key = B256::ZERO;
            key.0[0] = 0x20 | i;
            (key, U256::from(999))
        })
        .collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&blinded_keys);

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // Callback should fire for each key (3 invocations).
    assert_eq!(targets.len(), 3, "callback should fire once per key hitting the blinded node");
    // All updates should remain in the map for retry.
    assert_eq!(leaf_updates.len(), 3, "all keys should remain in map after blinded hit");
}

/// `LeafUpdate::Touched` on a fully revealed path should be a no-op.
pub(super) fn test_update_leaves_touched_fully_revealed<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    let root_before = trie.root();

    // Call update_leaves with Touched for an existing key.
    let mut leaf_updates: B256Map<LeafUpdate> = once((key2, LeafUpdate::Touched)).collect();

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // No callback should fire.
    assert!(targets.is_empty(), "no callback should fire for Touched on fully revealed path");
    // Key should be drained from the map.
    assert!(leaf_updates.is_empty(), "Touched key should be drained from updates map");
    // Root should be unchanged.
    assert_eq!(trie.root(), root_before, "root should be unchanged after Touched no-op");
}

/// `LeafUpdate::Touched` on a path with a blinded node should
/// invoke the callback and keep the key in the updates map. No trie mutation should occur.
pub(super) fn test_update_leaves_touched_blinded_requests_proof<T: SparseTrie + Default>() {
    // Two groups of 16 keys each under different first nibbles so that branch children
    // become hash nodes (>32 bytes RLP). Partial reveal leaves one subtrie blinded.
    let mut base_storage = BTreeMap::new();

    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage);

    // Reveal only group_a keys, leaving group_b's subtrie blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    let root_before = trie.root();

    // Submit Touched for a key in group_b's blinded subtrie.
    let target_key = {
        let mut key = B256::ZERO;
        key.0[0] = 0x20;
        key
    };
    let mut leaf_updates: B256Map<LeafUpdate> = once((target_key, LeafUpdate::Touched)).collect();

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    // Callback should have been invoked for the blinded node.
    assert!(!targets.is_empty(), "callback should fire for Touched on blinded path");
    // Key should remain in the updates map.
    assert!(!leaf_updates.is_empty(), "Touched key should remain in map when blinded");
    // Root should be unchanged (no mutation).
    assert_eq!(trie.root(), root_before, "root should be unchanged after Touched on blinded path");
}

/// Touched on a nonexistent key in an empty trie is drained silently.
///
/// An empty (default) trie has an Empty root — all paths are accessible (no blinded nodes).
/// `Touched` on a key that doesn't exist should be drained from the map without any
/// callback invocation or mutation.
pub(super) fn test_update_leaves_touched_nonexistent_key<T: SparseTrie + Default>() {
    let mut trie = T::default();

    let target_key = B256::with_last_byte(42);
    let mut leaf_updates: B256Map<LeafUpdate> = once((target_key, LeafUpdate::Touched)).collect();

    let mut callback_count = 0usize;
    trie.update_leaves(&mut leaf_updates, |_key, _min_len| {
        callback_count += 1;
    })
    .expect("update_leaves should succeed");

    assert!(leaf_updates.is_empty(), "Touched key should be drained from map");
    assert_eq!(callback_count, 0, "no callback should fire for Touched on empty trie");
    assert_eq!(
        trie.get_leaf_value(&Nibbles::unpack(target_key)),
        None,
        "nonexistent key should return None"
    );
}

/// `LeafUpdate::Touched` on a nonexistent key in a fully
/// revealed, populated trie should be a no-op — no callback, key drained, trie unchanged.
pub(super) fn test_update_leaves_touched_nonexistent_in_populated_trie<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    let root_before = trie.root();

    // Key 0x50 does not exist in the trie but its path is fully revealed (no blinded nodes).
    let nonexistent_key = B256::with_last_byte(0x50);
    let mut leaf_updates: B256Map<LeafUpdate> =
        once((nonexistent_key, LeafUpdate::Touched)).collect();

    let mut callback_count = 0usize;
    trie.update_leaves(&mut leaf_updates, |_key, _min_len| {
        callback_count += 1;
    })
    .expect("update_leaves should succeed");

    assert!(leaf_updates.is_empty(), "Touched key should be drained from updates map");
    assert_eq!(callback_count, 0, "no callback should fire for Touched on revealed path");
    assert_eq!(trie.root(), root_before, "root should be unchanged after Touched no-op");
    assert_eq!(
        trie.get_leaf_value(&Nibbles::unpack(nonexistent_key)),
        None,
        "nonexistent key should return None"
    );
}

/// A single `update_leaves` call with a mix of inserts,
/// modifications, removals, and touched entries should process all correctly.
pub(super) fn test_update_leaves_multiple_mixed_updates<T: SparseTrie + Default>() {
    let key_a = B256::with_last_byte(0x10); // will be inserted (new key)
    let key_b = B256::with_last_byte(0x20); // will be modified
    let key_c = B256::with_last_byte(0x30); // will be removed
    let key_d = B256::with_last_byte(0x40); // will be touched (no-op)
    let key_e = B256::with_last_byte(0x50); // stays unchanged

    let base_storage: BTreeMap<B256, U256> = [
        (key_b, U256::from(2)),
        (key_c, U256::from(3)),
        (key_d, U256::from(4)),
        (key_e, U256::from(5)),
    ]
    .into_iter()
    .collect();

    let mut harness = SuiteTestHarness::new(base_storage);

    // Fully reveal existing trie, plus proof for key_a (new key to be inserted).
    let all_keys = vec![key_a, key_b, key_c, key_d, key_e];
    let mut trie: T = harness.init_trie_with_targets(&all_keys, false);

    // Build mixed leaf updates.
    let new_value_a = U256::from(100);
    let updated_value_b = U256::from(999);
    let mut leaf_updates: B256Map<LeafUpdate> = [
        (key_a, LeafUpdate::Changed(encode_fixed_size(&new_value_a).to_vec())),
        (key_b, LeafUpdate::Changed(encode_fixed_size(&updated_value_b).to_vec())),
        (key_c, LeafUpdate::Changed(Vec::new())), // removal
        (key_d, LeafUpdate::Touched),
    ]
    .into_iter()
    .collect();

    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    assert!(leaf_updates.is_empty(), "all keys should be drained from the updates map");

    // Build reference trie: A inserted, B modified, C removed, D and E unchanged.
    let expected_storage: BTreeMap<B256, U256> = [
        (key_a, new_value_a),
        (key_b, updated_value_b),
        // key_c removed
        (key_d, U256::from(4)),
        (key_e, U256::from(5)),
    ]
    .into_iter()
    .collect();

    harness.apply_changeset(expected_storage.clone());
    // apply_changeset merged into existing storage — need a fresh harness for the expected state.
    let expected_harness = SuiteTestHarness::new(expected_storage);

    let root = trie.root();
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie with mixed updates applied"
    );
}

/// Ancestors marked dirty even without cached hashes.
///
/// When removing a leaf, all ancestor nodes must be marked dirty unconditionally —
/// not just those that previously had a cached hash. This test inserts leaves without
/// calling `root()` (so no hashes are cached), then removes a leaf and verifies
/// `root()` returns the correct hash.
pub(super) fn test_remove_leaf_marks_ancestors_dirty_unconditionally<T: SparseTrie + Default>() {
    // Create a trie with 5 leaves.
    let mut keys = Vec::new();
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..5 {
        let mut key = B256::ZERO;
        key.0[0] = (i + 1) * 16; // 0x10, 0x20, 0x30, 0x40, 0x50
        storage.insert(key, U256::from(i as u64 + 1));
        keys.push(key);
    }

    let harness = SuiteTestHarness::new(storage.clone());

    // Initialize trie: set root and reveal all proofs.
    let root_node = harness.root_node();
    let mut trie = T::default();
    trie.set_root(root_node.node, root_node.masks, false).expect("set_root should succeed");

    let mut targets: Vec<ProofV2Target> = keys.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");

    // Insert all leaves via update_leaves — do NOT call root() so no hashes are cached.
    let mut insert_updates = SuiteTestHarness::leaf_updates(&storage);
    trie.update_leaves(&mut insert_updates, |_, _| {})
        .expect("insert update_leaves should succeed");

    // Now remove one leaf WITHOUT having called root() first.
    let mut removal: BTreeMap<B256, U256> = BTreeMap::new();
    removal.insert(keys[2], U256::ZERO); // remove key 0x30
    let mut removal_updates = SuiteTestHarness::leaf_updates(&removal);
    trie.update_leaves(&mut removal_updates, |_, _| {})
        .expect("removal update_leaves should succeed");

    // Call root() — all ancestors of the removed leaf must be marked dirty
    // even though no hashes were previously cached.
    let root = trie.root();

    // Build reference trie with same final state.
    let mut expected_storage = storage;
    expected_storage.remove(&keys[2]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference after removal without prior root() call"
    );
}

/// Orphaned value update falls through to full insertion.
///
/// After a sequence of insertions and removals that involve branch collapses,
/// every key with a value must remain findable via `find_leaf` and updatable
/// via `update_leaves`. This verifies the invariant that structural consistency
/// is maintained even when branch collapses could orphan value entries.
pub(super) fn test_orphaned_value_update_falls_through_to_full_insertion<
    T: SparseTrie + Default,
>() {
    // Create a trie with 3 leaves sharing a branch prefix, plus 2 additional leaves
    // in different subtries. Keys chosen so removal of key_c collapses the branch
    // at the shared prefix.
    let key_a = {
        let mut k = B256::ZERO;
        k.0[0] = 0x10;
        k.0[1] = 0x00;
        k
    };
    let key_b = {
        let mut k = B256::ZERO;
        k.0[0] = 0x10;
        k.0[1] = 0x10;
        k
    };
    let key_c = {
        let mut k = B256::ZERO;
        k.0[0] = 0x10;
        k.0[1] = 0x20;
        k
    };
    let key_d = {
        let mut k = B256::ZERO;
        k.0[0] = 0x20;
        k
    };
    let key_e = {
        let mut k = B256::ZERO;
        k.0[0] = 0x30;
        k
    };

    let initial_storage: BTreeMap<B256, U256> = [
        (key_a, U256::from(1)),
        (key_b, U256::from(2)),
        (key_c, U256::from(3)),
        (key_d, U256::from(4)),
        (key_e, U256::from(5)),
    ]
    .into_iter()
    .collect();

    let mut harness = SuiteTestHarness::new(initial_storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Insert all leaves.
    let mut insert_updates = SuiteTestHarness::leaf_updates(&initial_storage);
    harness.reveal_and_update(&mut trie, &mut insert_updates);
    let root1 = trie.root();
    assert_eq!(root1, harness.original_root(), "initial root should match");

    // Step 1: Remove key_c to collapse the branch at 0x10..
    let removal: BTreeMap<B256, U256> = once((key_c, U256::ZERO)).collect();
    harness.apply_changeset(removal.clone());
    let mut removal_updates = SuiteTestHarness::leaf_updates(&removal);
    harness.reveal_and_update(&mut trie, &mut removal_updates);
    let root2 = trie.root();
    assert_eq!(root2, harness.original_root(), "root after removal should match");

    // Step 2: Re-insert key_c with a new value — this re-creates the branch.
    let reinsert: BTreeMap<B256, U256> = once((key_c, U256::from(33))).collect();
    harness.apply_changeset(reinsert.clone());
    let mut reinsert_updates = SuiteTestHarness::leaf_updates(&reinsert);
    harness.reveal_and_update(&mut trie, &mut reinsert_updates);
    let root3 = trie.root();
    assert_eq!(root3, harness.original_root(), "root after re-insert should match");

    // Step 3: Update key_a — previously could be orphaned if branch collapse
    // didn't maintain structural tracking properly.
    let update: BTreeMap<B256, U256> = once((key_a, U256::from(999))).collect();
    harness.apply_changeset(update.clone());
    let mut update_updates = SuiteTestHarness::leaf_updates(&update);
    harness.reveal_and_update(&mut trie, &mut update_updates);
    let root4 = trie.root();
    assert_eq!(root4, harness.original_root(), "root after updating key_a should match");

    // Verify the invariant: every key with a value must be findable via find_leaf.
    let final_keys = [key_a, key_b, key_c, key_d, key_e];
    for key in &final_keys {
        let nibbles = Nibbles::unpack(*key);
        assert_eq!(
            trie.find_leaf(&nibbles, None).expect("find_leaf should succeed"),
            LeafLookup::Exists,
            "key {key} should be findable after collapse-reinsert-update sequence"
        );
    }

    // Also verify a nonexistent key is properly reported.
    let nonexistent = B256::with_last_byte(0xFF);
    let nonexistent_nibbles = Nibbles::unpack(nonexistent);
    assert_eq!(
        trie.find_leaf(&nonexistent_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "nonexistent key should not be found"
    );
}

/// When removing a leaf causes a branch to collapse at a subtrie
/// boundary, the remaining sibling leaf's `key_len` metadata must be updated. After the
/// collapse the remaining leaf must be findable, updatable, and contribute to the correct root.
pub(super) fn test_branch_collapse_updates_leaf_key_len_across_subtries<T: SparseTrie + Default>() {
    // Create two leaves that share a branch at a subtrie boundary.
    // Keys share the same first nibble (0x1) so they form a branch one level down,
    // which is a subtrie boundary for the parallel sparse trie.
    let key_a = B256::with_last_byte(0x10); // nibbles: ...1, 0
    let key_b = B256::with_last_byte(0x12); // nibbles: ...1, 2

    let base_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let mut harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Step 1: Remove key_a → branch collapses, key_b becomes the sole child.
    let removal: BTreeMap<B256, U256> = once((key_a, U256::ZERO)).collect();
    harness.apply_changeset(removal.clone());
    let mut removal_updates = SuiteTestHarness::leaf_updates(&removal);
    harness.reveal_and_update(&mut trie, &mut removal_updates);

    let root_after_removal = trie.root();
    assert_eq!(
        root_after_removal,
        harness.original_root(),
        "root after branch collapse should match reference"
    );

    // Verify remaining leaf is findable.
    let key_b_nibbles = Nibbles::unpack(key_b);
    assert_eq!(
        trie.find_leaf(&key_b_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::Exists,
        "remaining leaf (key_b) should be findable after branch collapse"
    );

    // Step 2: Modify the remaining leaf's value — this must succeed after the collapse
    // updated key_len properly.
    let modification: BTreeMap<B256, U256> = once((key_b, U256::from(999))).collect();
    harness.apply_changeset(modification.clone());
    let mut mod_updates = SuiteTestHarness::leaf_updates(&modification);
    harness.reveal_and_update(&mut trie, &mut mod_updates);

    let root_after_update = trie.root();
    assert_eq!(
        root_after_update,
        harness.original_root(),
        "root after updating remaining leaf should match reference"
    );
}

/// Removal of a leaf should never corrupt blinded/pruned subtries.
///
/// When a branch collapses during leaf removal and the remaining child's value needs to be
/// moved between subtries, the operation must not reveal (initialize) blind/unloaded subtries.
/// Either the removal succeeds cleanly or it requests proofs for the blinded area.
pub(super) fn test_remove_leaf_does_not_reveal_blind_subtries<T: SparseTrie + Default>() {
    // Create a trie with 10 leaves across different first-nibble subtries.
    // We'll prune some subtries, then remove a leaf whose branch collapse
    // involves a sibling in a pruned (blinded) subtrie.
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    let mut keys = Vec::new();
    for i in 0u8..10 {
        let mut key = B256::ZERO;
        key.0[0] = i * 16; // nibble prefixes: 0x0, 0x1, ..., 0x9
        storage.insert(key, U256::from(i as u64 + 1));
        keys.push(key);
    }

    let mut harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute initial root and commit to establish baseline.
    let _ = trie.root();
    let updates = trie.take_updates();
    trie.commit_updates(&updates.updated_nodes, &updates.removed_nodes);

    // Prune: retain only keys[0] and keys[1] (nibbles 0x0 and 0x1).
    // All other subtries become blinded.
    let retained: Vec<Nibbles> = [keys[0], keys[1]].iter().map(|k| Nibbles::unpack(*k)).collect();
    trie.prune(&retained);

    // Verify root is unchanged after prune.
    let root_after_prune = trie.root();
    assert_eq!(root_after_prune, harness.original_root(), "root should be unchanged after prune");

    // Now remove keys[0] — this leaves keys[1] and all the blinded subtries.
    // The branch at the root still has multiple children (keys[1] + blinded children),
    // so this should not trigger a problematic branch collapse into blinded territory.
    let removal: BTreeMap<B256, U256> = once((keys[0], U256::ZERO)).collect();
    harness.apply_changeset(removal.clone());
    let mut removal_updates = SuiteTestHarness::leaf_updates(&removal);
    harness.reveal_and_update(&mut trie, &mut removal_updates);

    let root_after_removal = trie.root();
    assert_eq!(
        root_after_removal,
        harness.original_root(),
        "root after removing a revealed leaf with blinded sibling subtries should match reference"
    );

    // Verify that the removed leaf is gone.
    let removed_nibbles = Nibbles::unpack(keys[0]);
    let find_result = trie.find_leaf(&removed_nibbles, None).expect("find_leaf should succeed");
    assert_eq!(find_result, LeafLookup::NonExistent, "removed leaf should not be found");

    // Verify that the retained leaf (keys[1]) is still accessible.
    let retained_nibbles = Nibbles::unpack(keys[1]);
    let find_result = trie.find_leaf(&retained_nibbles, None).expect("find_leaf should succeed");
    assert_eq!(find_result, LeafLookup::Exists, "retained leaf should still be findable");

    // Now update the retained leaf to verify the trie is in a consistent state.
    let modification: BTreeMap<B256, U256> = once((keys[1], U256::from(999))).collect();
    harness.apply_changeset(modification.clone());
    let mut mod_updates = SuiteTestHarness::leaf_updates(&modification);
    harness.reveal_and_update(&mut trie, &mut mod_updates);

    let root_after_mod = trie.root();
    assert_eq!(
        root_after_mod,
        harness.original_root(),
        "root after modifying retained leaf should match reference"
    );
}
