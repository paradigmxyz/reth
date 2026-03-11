use super::*;

/// Empty slice is a no-op.
///
/// Calling `reveal_nodes` with an empty slice should return `Ok(())` and leave
/// the trie state unchanged.
pub(super) fn test_reveal_nodes_empty_slice<T: SparseTrie + Default>() {
    // Set up a trie with a root node.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2))]);

    let harness = SuiteTestHarness::new(storage);
    let root_node = harness.root_node();
    let mut trie = T::default();
    trie.set_root(root_node.node, root_node.masks, true).expect("set_root should succeed");

    let root_before = trie.root();

    // Call reveal_nodes with an empty slice — should be a no-op.
    trie.reveal_nodes(&mut []).expect("reveal_nodes with empty slice should succeed");

    let root_after = trie.root();
    assert_eq!(root_before, root_after, "root should be unchanged after empty reveal_nodes");
}

/// Single leaf reveal produces correct root.
///
/// Revealing a single leaf node within a branch should make it accessible and
/// produce correct root hashes.
pub(super) fn test_reveal_nodes_single_leaf<T: SparseTrie + Default>() {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);

    // Set root and reveal only one leaf's proof.
    let mut trie: T = harness.init_trie_with_targets(&[key_a], true);
    let root = trie.root();
    assert_eq!(root, harness.original_root());
}

/// Double reveal doesn't corrupt state.
///
/// Revealing the same proof nodes twice should not corrupt the trie or change
/// the root hash. The second reveal is a no-op.
pub(super) fn test_reveal_nodes_idempotent<T: SparseTrie + Default>() {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);

    // First reveal: set root and reveal all proof nodes.
    let mut trie: T = harness.init_trie_fully_revealed(true);
    let root_first = trie.root();
    assert_eq!(root_first, harness.original_root());

    // Second reveal: reveal the same proof nodes again.
    let keys: Vec<B256> = harness.storage().keys().copied().collect();
    let mut targets: Vec<ProofV2Target> = keys.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("second reveal_nodes should succeed");

    let root_second = trie.root();
    assert_eq!(root_first, root_second, "root should be unchanged after double reveal");
}

/// Masks guide update tracking.
///
/// Branch node masks provided during reveal should be stored and used for update tracking.
/// After modifying a leaf and computing the root, `take_updates()` should contain entries
/// reflecting which branch nodes were updated vs removed, guided by the stored masks.
pub(super) fn test_reveal_nodes_with_branch_masks<T: SparseTrie + Default>() {
    // Build a trie with 16 leaves sharing first nibble 0x1 to produce non-root branch nodes
    // with hashed children (needed for masks to produce InsertUpdated actions).
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);

    // Initialize trie with masks (from proofs) and retain_updates=true.
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Compute root to cache initial branch hashes.
    let _ = trie.root();

    // Add a new leaf under the same prefix so non-root branch nodes change.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    let changeset: BTreeMap<B256, U256> = BTreeMap::from([(new_key, U256::from(999))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root();

    // Updates should reflect which branch nodes were updated vs removed,
    // guided by the masks stored during reveal_nodes.
    let updates = trie.take_updates();
    assert!(
        !updates.updated_nodes.is_empty() || !updates.removed_nodes.is_empty(),
        "take_updates should be non-empty when masks guide update tracking: updated={}, removed={}",
        updates.updated_nodes.len(),
        updates.removed_nodes.len(),
    );
}

/// Reveal on empty root is a silent no-op.
///
/// Calling `reveal_nodes` when the root is `EmptyRoot` should return `Ok(())` without
/// modifying trie state, even when non-empty proof nodes are provided.
pub(super) fn test_reveal_nodes_skips_on_empty_root<T: SparseTrie + Default>() {
    // Build a harness with real data so we can obtain non-trivial proof nodes.
    let storage: BTreeMap<B256, U256> = BTreeMap::from([
        (B256::with_last_byte(1), U256::from(10)),
        (B256::with_last_byte(2), U256::from(20)),
    ]);
    let harness = SuiteTestHarness::new(storage);

    let keys: Vec<B256> = harness.storage().keys().copied().collect();
    let mut targets: Vec<ProofV2Target> = keys.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);

    // Create a trie with an empty root.
    let mut trie = T::default();
    trie.set_root(TrieNodeV2::EmptyRoot, None, true).expect("set_root EmptyRoot should succeed");

    // Reveal non-empty proof nodes — should be a no-op on an empty root.
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes on empty root should succeed");

    // Trie should still be empty.
    assert_eq!(
        trie.root(),
        EMPTY_ROOT_HASH,
        "root should remain EMPTY_ROOT_HASH after reveal on empty root"
    );
}

/// Extra/unreachable proof nodes don't corrupt state.
///
/// When `reveal_nodes` receives proof nodes that include entries not reachable from the
/// current trie root (e.g., boundary leaves for unrelated subtries), those nodes should
/// be silently skipped without corrupting state.
pub(super) fn test_reveal_nodes_filters_unreachable_boundary_leaves<T: SparseTrie + Default>() {
    // Create a trie with two groups of keys under different first nibbles.
    // Group A: 3 keys under nibble 0x1
    // Group B: 3 keys under nibble 0x2
    let mut keys_a = Vec::new();
    for i in 0u8..3 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        keys_a.push(key);
    }
    let mut keys_b = Vec::new();
    for i in 0u8..3 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20;
        key.0[1] = i * 16;
        keys_b.push(key);
    }

    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for (i, key) in keys_a.iter().enumerate() {
        storage.insert(*key, U256::from(i as u64 + 1));
    }
    for (i, key) in keys_b.iter().enumerate() {
        storage.insert(*key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(storage);

    // Initialize trie with root and reveal ONLY group A keys.
    let root_node = harness.root_node();
    let mut trie = T::default();
    trie.set_root(root_node.node, root_node.masks, false).expect("set_root should succeed");

    let mut targets_a: Vec<ProofV2Target> = keys_a.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_a, _) = harness.proof_v2(&mut targets_a);
    trie.reveal_nodes(&mut proof_a).expect("reveal group A should succeed");

    // Verify root is correct with partial reveal.
    let root_after_a = trie.root();
    assert_eq!(root_after_a, harness.original_root(), "root after group A reveal should match");

    // Now generate proofs for group B keys and reveal them as extra nodes.
    // These include boundary leaves that extend the trie into subtries the partial
    // reveal didn't cover. They should be handled gracefully.
    let mut targets_b: Vec<ProofV2Target> = keys_b.iter().map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_b, _) = harness.proof_v2(&mut targets_b);
    trie.reveal_nodes(&mut proof_b).expect("reveal extra group B nodes should succeed");

    // Root must still be correct — extra reveals should not corrupt state.
    let root_after_b = trie.root();
    assert_eq!(
        root_after_b,
        harness.original_root(),
        "root should be unchanged after extra reveal"
    );

    // Additionally, reveal ALL proofs at once (including duplicates) — still correct.
    let mut targets_all: Vec<ProofV2Target> =
        keys_a.iter().chain(keys_b.iter()).map(|k| ProofV2Target::new(*k)).collect();
    let (mut proof_all, _) = harness.proof_v2(&mut targets_all);
    trie.reveal_nodes(&mut proof_all).expect("reveal all nodes should succeed");

    let root_after_all = trie.root();
    assert_eq!(
        root_after_all,
        harness.original_root(),
        "root should be unchanged after revealing all proofs"
    );
}

/// Insert between reveals doesn't lose state.
///
/// When proofs from a 2-leaf trie are revealed, then a 3rd leaf is inserted, then another
/// proof from the original 2-leaf trie is revealed, the branch node should not be overwritten
/// by the stale proof. The root must match a reference trie with all 3 keys.
pub(super) fn test_reveal_insert_reveal_preserves_branch_state<T: SparseTrie + Default>() {
    // Two original keys and one to insert.
    let key_a = B256::with_last_byte(0x00);
    let key_b = B256::with_last_byte(0x01);
    let key_c = B256::with_last_byte(0x02);

    let original_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(original_storage);

    // Initialize trie with root, reveal proof for key_a only.
    let mut trie: T = harness.init_trie_with_targets(&[key_a], false);

    // Insert key_b via update_leaves.
    let insert_value = U256::from(2);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_b, insert_value)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after insert");

    // Reveal proof for key_c from the *original* 2-leaf harness (stale proof).
    let mut targets = vec![ProofV2Target::new(key_c)];
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal stale proof for key_c should succeed");

    // Root must match a reference trie with all 3 keys.
    let root = trie.root();
    let expected_storage =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, insert_value), (key_c, U256::from(3))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference trie with all 3 keys after stale reveal"
    );
}

/// After removing a leaf that collapses a branch into an
/// extension, revealing a stale proof (which had a branch at root) should not overwrite the
/// extension node.
pub(super) fn test_remove_then_reveal_does_not_overwrite_collapsed_node<T: SparseTrie + Default>() {
    // Nibbles [0,0,..], [1,1,..], [1,2,..] — root branch has children at nibbles 0 and 1.
    // Packed into B256 keys: byte 0x00 → nibbles [0,0], byte 0x11 → nibbles [1,1], etc.
    let key_a = {
        let mut k = B256::ZERO;
        k.0[0] = 0x00;
        k
    };
    let key_b = {
        let mut k = B256::ZERO;
        k.0[0] = 0x11;
        k
    };
    let key_c = {
        let mut k = B256::ZERO;
        k.0[0] = 0x12;
        k
    };

    let original_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(original_storage);

    // Initialize trie with root and reveal proofs for all keys.
    let mut trie: T = harness.init_trie_with_targets(&[key_a, key_b, key_c], false);

    // Remove key_a (0x0000..) — should collapse root branch into extension (shared prefix 0x01).
    let removals: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::ZERO)]);
    let mut removal_updates = SuiteTestHarness::leaf_updates(&removals);
    harness.reveal_and_update(&mut trie, &mut removal_updates);
    assert!(removal_updates.is_empty(), "removal should be drained");

    // Reveal stale proof for key_b from the *original* 3-key harness (which has a branch at root).
    let mut targets = vec![ProofV2Target::new(key_b)];
    let (mut stale_proof, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut stale_proof).expect("revealing stale proof should succeed");

    // Root must match a reference trie with only key_b and key_c.
    let root = trie.root();
    let expected_storage = BTreeMap::from([(key_b, U256::from(2)), (key_c, U256::from(3))]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match 2-key reference after removal + stale reveal"
    );
}

/// After inserting a leaf that converts an extension root into
/// a branch, revealing a stale proof from the original trie (which has an extension at root)
/// should not overwrite the branch.
pub(super) fn test_insert_then_reveal_does_not_overwrite_branch<T: SparseTrie + Default>() {
    // Original trie: keys 0x0001.. and 0x0002.. share prefix 0x00 → extension root.
    let key_a = {
        let mut k = B256::ZERO;
        k.0[0] = 0x00;
        k.0[1] = 0x01;
        k
    };
    let key_b = {
        let mut k = B256::ZERO;
        k.0[0] = 0x00;
        k.0[1] = 0x02;
        k
    };

    let original_storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2))]);

    let harness = SuiteTestHarness::new(original_storage);

    // Initialize trie with root, reveal all proofs.
    let mut trie: T = harness.init_trie_with_targets(&[key_a, key_b], false);

    // Insert key_c at 0x0100.. — different first nibble, forces extension→branch conversion.
    let key_c = {
        let mut k = B256::ZERO;
        k.0[0] = 0x01;
        k
    };
    let insert_value = U256::from(3);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&BTreeMap::from([(key_c, insert_value)]));
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    assert!(leaf_updates.is_empty(), "leaf_updates should be drained after insert");

    // Reveal stale proof for key_a from the *original* 2-key harness (which has extension at root).
    let mut targets = vec![ProofV2Target::new(key_a)];
    let (mut stale_proof, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut stale_proof).expect("revealing stale proof should succeed");

    // Root must match a reference trie with all 3 keys.
    let root = trie.root();
    let expected_storage =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, insert_value)]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match 3-key reference after insert + stale reveal"
    );
}
