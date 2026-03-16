use super::*;

/// Standard production lifecycle — update, root, `take_updates`, `commit_updates`.
///
/// Build a trie with enough leaves to produce hashed branch children (≥16 per subtrie),
/// insert 1 new leaf + modify 1 existing, compute root, take updates, commit, then verify
/// root is unchanged (cache hit) and updates are non-empty.
pub(super) fn test_full_lifecycle_update_root_take_commit<T: SparseTrie + Default>() {
    // 16 leaves sharing prefix [1,0] to produce hashed branch children.
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        storage.insert(key, U256::from(i as u64 + 1));
    }
    // Extra leaf under a different nibble.
    let mut key_extra = B256::ZERO;
    key_extra.0[0] = 0x20;
    storage.insert(key_extra, U256::from(100));

    let harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Cache initial hashes.
    let _ = trie.root();

    // Insert 1 new leaf under the same hashed prefix and modify 1 existing leaf.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF; // new leaf under the [1,0] prefix
    let mut modify_key = B256::ZERO;
    modify_key.0[0] = 0x10;
    modify_key.0[1] = 0x00; // first key in the group

    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    changeset.insert(new_key, U256::from(999)); // new leaf
    changeset.insert(modify_key, U256::from(500)); // modify existing

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let hash1 = trie.root();

    // Verify hash1 matches the reference trie with updated values.
    let mut expected_storage = storage;
    expected_storage.insert(new_key, U256::from(999));
    expected_storage.insert(modify_key, U256::from(500));
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(hash1, expected_harness.original_root(), "root should match reference trie");

    // Take updates — should be non-empty with hashed branch children.
    let updates = trie.take_updates();
    assert!(
        !updates.updated_nodes.is_empty() || !updates.removed_nodes.is_empty(),
        "updates should be non-empty after mutations"
    );

    // Commit updates.
    trie.commit_updates(&updates.updated_nodes, &updates.removed_nodes);

    // Post-commit root should still be hash1 (cache hit).
    let hash2 = trie.root();
    assert_eq!(hash1, hash2, "root should be unchanged after commit_updates");
}

/// Multiple rounds of (update → root → `take_updates` → `commit_updates`), followed by
/// a prune, simulating block processing.
pub(super) fn test_multi_round_update_commit_prune_cycle<T: SparseTrie + Default>() {
    // Build a trie with 10 leaves.
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    let mut keys = Vec::new();
    for i in 0u8..10 {
        let mut key = B256::ZERO;
        key.0[0] = i * 16; // nibble prefixes: 0x0, 0x1, 0x2, ... 0x9
        storage.insert(key, U256::from(i as u64 + 1));
        keys.push(key);
    }

    let mut harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Cache initial hashes.
    let _ = trie.root();

    // --- Round 1: Update leaves A (keys[0]) and B (keys[1]) ---
    let mut changeset1: BTreeMap<B256, U256> = BTreeMap::new();
    changeset1.insert(keys[0], U256::from(100));
    changeset1.insert(keys[1], U256::from(200));
    let mut leaf_updates1 = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates1);
    let root1 = trie.root();

    // Verify root1 matches reference.
    harness.apply_changeset(changeset1);
    assert_eq!(root1, harness.original_root(), "round 1 root should match reference");

    let updates1 = trie.take_updates();
    trie.commit_updates(&updates1.updated_nodes, &updates1.removed_nodes);

    // --- Round 2: Update leaves C (keys[2]) and D (keys[3]) ---
    let mut changeset2: BTreeMap<B256, U256> = BTreeMap::new();
    changeset2.insert(keys[2], U256::from(300));
    changeset2.insert(keys[3], U256::from(400));
    let mut leaf_updates2 = SuiteTestHarness::leaf_updates(&changeset2);
    harness.reveal_and_update(&mut trie, &mut leaf_updates2);
    let root2 = trie.root();

    // Verify root2 matches reference.
    harness.apply_changeset(changeset2);
    assert_eq!(root2, harness.original_root(), "round 2 root should match reference");

    let updates2 = trie.take_updates();
    trie.commit_updates(&updates2.updated_nodes, &updates2.removed_nodes);

    // --- Prune: retain only keys A,B,C,D ---
    let retained: Vec<Nibbles> = keys[0..4].iter().map(|k| Nibbles::unpack(*k)).collect();
    let pre_prune_size = trie.size_hint();
    trie.prune(&retained);
    let post_prune_size = trie.size_hint();
    if pre_prune_size > 0 {
        assert!(post_prune_size < pre_prune_size, "prune should reduce node count");
    }

    // Root should still be correct after prune.
    let root_after_prune = trie.root();
    assert_eq!(root_after_prune, harness.original_root(), "root should be unchanged after prune");

    // --- Round 3: Update leaf E (keys[4]) — needs re-reveal since it was pruned ---
    let mut changeset3: BTreeMap<B256, U256> = BTreeMap::new();
    changeset3.insert(keys[4], U256::from(500));
    let mut leaf_updates3 = SuiteTestHarness::leaf_updates(&changeset3);
    harness.reveal_and_update(&mut trie, &mut leaf_updates3);
    let root3 = trie.root();

    // Verify root3 matches reference.
    harness.apply_changeset(changeset3);
    assert_eq!(
        root3,
        harness.original_root(),
        "round 3 root should match reference after re-reveal"
    );
}

/// Core production loop: reveal → update → root.
///
/// Build a 5-leaf trie, reveal all proofs, apply 2 modifications + 1 removal,
/// and verify root matches the reference trie with the same mutations.
pub(super) fn test_reveal_update_root_basic_lifecycle<T: SparseTrie + Default>() {
    let mut keys = Vec::new();
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..5 {
        let mut key = B256::ZERO;
        key.0[0] = (i + 1) * 16; // 0x10, 0x20, 0x30, 0x40, 0x50
        storage.insert(key, U256::from(i as u64 + 1));
        keys.push(key);
    }

    let harness = SuiteTestHarness::new(storage.clone());
    let mut trie: T = harness.init_trie_fully_revealed(true);

    // Apply 2 modifications and 1 removal.
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    changeset.insert(keys[0], U256::from(100)); // modify key 0x10
    changeset.insert(keys[1], U256::from(200)); // modify key 0x20
    changeset.insert(keys[2], U256::ZERO); // remove key 0x30

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let root = trie.root();

    // Build reference trie with same mutations.
    let mut expected_storage = storage;
    expected_storage.insert(keys[0], U256::from(100));
    expected_storage.insert(keys[1], U256::from(200));
    expected_storage.remove(&keys[2]);
    let expected_harness = SuiteTestHarness::new(expected_storage);
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference after modifications and removal"
    );
}

/// Incremental reveal and update with retry loop.
/// Partial proof → `update_leaves` hits blinded nodes → reveal more → retry succeeds.
pub(super) fn test_incremental_reveal_and_update_with_retry<T: SparseTrie + Default>() {
    // Build 10 leaves across multiple subtries so partial reveal leaves some blinded.
    // Use 16 keys per group so branch children become hash nodes (>32 bytes RLP).
    let mut base_storage = BTreeMap::new();

    // Group A: 16 keys under nibble 0x1
    let mut group_a_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10 | i;
        group_a_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 1));
    }

    // Group B: 16 keys under nibble 0x2
    let mut group_b_keys = Vec::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x20 | i;
        group_b_keys.push(key);
        base_storage.insert(key, U256::from(i as u64 + 100));
    }

    let harness = SuiteTestHarness::new(base_storage.clone());

    // Reveal only group A keys, leaving group B's subtrie blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, true);

    // Prepare updates for 5 keys: 3 from group A (covered) + 2 from group B (blinded).
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    changeset.insert(group_a_keys[0], U256::from(500)); // modify covered key
    changeset.insert(group_a_keys[1], U256::from(600)); // modify covered key
    changeset.insert(group_a_keys[2], U256::from(700)); // modify covered key
    changeset.insert(group_b_keys[0], U256::from(800)); // modify blinded key
    changeset.insert(group_b_keys[1], U256::from(900)); // modify blinded key

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);

    // First update_leaves: covered keys are drained, blinded keys remain, callback fires.
    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed");

    assert!(!targets.is_empty(), "callback should fire for blinded keys");
    assert!(!leaf_updates.is_empty(), "blinded keys should remain in updates map");

    // Reveal the proof for the requested targets.
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");

    // Second update_leaves: now all paths are revealed, remaining keys should be drained.
    let mut targets2: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets2.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves should succeed on retry");

    assert!(targets2.is_empty(), "no callback should fire after reveal");
    assert!(leaf_updates.is_empty(), "all keys should be drained after retry");

    // Root should match reference with all 5 updates applied.
    let mut expected_storage = base_storage;
    expected_storage.insert(group_a_keys[0], U256::from(500));
    expected_storage.insert(group_a_keys[1], U256::from(600));
    expected_storage.insert(group_a_keys[2], U256::from(700));
    expected_storage.insert(group_b_keys[0], U256::from(800));
    expected_storage.insert(group_b_keys[1], U256::from(900));
    let expected_harness = SuiteTestHarness::new(expected_storage);

    let root = trie.root();
    assert_eq!(
        root,
        expected_harness.original_root(),
        "root should match reference after incremental reveal and retry"
    );
}

/// End-to-end block processing with storage + account coordination.
///
/// Simulates a complete block processing cycle: receive state updates → apply to storage
/// tries → compute storage roots → promote to account trie → compute state root → take
/// updates → commit → prune for next block.
pub(super) fn test_full_block_processing_lifecycle<T: SparseTrie + Default>() {
    // --- Setup: Build account trie with 5 accounts ---
    // A1 storage: 5 slots, A2 storage: 3 slots, A3-A5: empty storage.
    // Account trie leaf values = RLP-encoded storage roots.

    // A1 storage trie: 5 slots
    let mut a1_storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..5 {
        let mut key = B256::ZERO;
        key.0[0] = (i + 1) * 16; // 0x10, 0x20, 0x30, 0x40, 0x50
        a1_storage.insert(key, U256::from(i as u64 + 1));
    }
    let mut a1_harness = SuiteTestHarness::new(a1_storage.clone());
    let a1_initial_root = a1_harness.original_root();

    // A2 storage trie: 3 slots
    let mut a2_storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..3 {
        let mut key = B256::ZERO;
        key.0[0] = (i + 1) * 16; // 0x10, 0x20, 0x30
        a2_storage.insert(key, U256::from(i as u64 + 10));
    }
    let mut a2_harness = SuiteTestHarness::new(a2_storage.clone());
    let a2_initial_root = a2_harness.original_root();

    // Account trie keys.
    let mut acct_keys = Vec::new();
    for i in 0u8..5 {
        let mut key = B256::ZERO;
        key.0[0] = (i + 1) * 16; // 0x10, 0x20, 0x30, 0x40, 0x50
        acct_keys.push(key);
    }

    // Account trie leaf values = RLP-encoded storage roots (or EMPTY_ROOT_HASH for A3-A5).
    let encode_root = |root: B256| -> U256 {
        // Use the first 32 bytes of the root hash as a U256 value for the account trie leaf.
        U256::from_be_bytes(root.0)
    };

    let mut acct_storage: BTreeMap<B256, U256> = BTreeMap::new();
    acct_storage.insert(acct_keys[0], encode_root(a1_initial_root)); // A1
    acct_storage.insert(acct_keys[1], encode_root(a2_initial_root)); // A2
    acct_storage.insert(acct_keys[2], encode_root(EMPTY_ROOT_HASH)); // A3
    acct_storage.insert(acct_keys[3], encode_root(EMPTY_ROOT_HASH)); // A4
    acct_storage.insert(acct_keys[4], encode_root(EMPTY_ROOT_HASH)); // A5

    let mut acct_harness = SuiteTestHarness::new(acct_storage.clone());

    // Initialize all tries fully revealed with update tracking.
    let mut a1_trie: T = a1_harness.init_trie_fully_revealed(true);
    let mut a2_trie: T = a2_harness.init_trie_fully_revealed(true);
    let mut acct_trie: T = acct_harness.init_trie_fully_revealed(true);

    // Cache initial hashes for all tries.
    let _ = a1_trie.root();
    let _ = a2_trie.root();
    let _ = acct_trie.root();

    // --- Storage phase ---
    // A1: modify S1 (key 0x10), remove S2 (key 0x20), add S6 (key 0x60).
    let mut a1_s1_key = B256::ZERO;
    a1_s1_key.0[0] = 0x10;
    let mut a1_s2_key = B256::ZERO;
    a1_s2_key.0[0] = 0x20;
    let mut a1_s6_key = B256::ZERO;
    a1_s6_key.0[0] = 0x60;

    let mut a1_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    a1_changeset.insert(a1_s1_key, U256::from(100)); // modify S1
    a1_changeset.insert(a1_s2_key, U256::ZERO); // remove S2
    a1_changeset.insert(a1_s6_key, U256::from(6)); // add S6
    let mut a1_leaf_updates = SuiteTestHarness::leaf_updates(&a1_changeset);
    a1_harness.reveal_and_update(&mut a1_trie, &mut a1_leaf_updates);

    // A2: modify S1 (key 0x10).
    let mut a2_s1_key = B256::ZERO;
    a2_s1_key.0[0] = 0x10;
    let mut a2_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    a2_changeset.insert(a2_s1_key, U256::from(200)); // modify S1
    let mut a2_leaf_updates = SuiteTestHarness::leaf_updates(&a2_changeset);
    a2_harness.reveal_and_update(&mut a2_trie, &mut a2_leaf_updates);

    // --- Storage root phase ---
    let sr1 = a1_trie.root();
    let sr2 = a2_trie.root();

    // Verify storage roots match references.
    a1_harness.apply_changeset(a1_changeset);
    assert_eq!(sr1, a1_harness.original_root(), "A1 storage root should match reference");

    a2_harness.apply_changeset(a2_changeset);
    assert_eq!(sr2, a2_harness.original_root(), "A2 storage root should match reference");

    // --- Account promotion phase ---
    // Encode A1 with sr1, A2 with sr2, modify A3 (balance change = different value).
    let mut acct_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    acct_changeset.insert(acct_keys[0], encode_root(sr1)); // A1 with new storage root
    acct_changeset.insert(acct_keys[1], encode_root(sr2)); // A2 with new storage root
    acct_changeset.insert(acct_keys[2], U256::from(42)); // A3 balance change
    let mut acct_leaf_updates = SuiteTestHarness::leaf_updates(&acct_changeset);
    acct_harness.reveal_and_update(&mut acct_trie, &mut acct_leaf_updates);

    // --- State root phase ---
    let state_root = acct_trie.root();

    // Verify state root matches reference.
    acct_harness.apply_changeset(acct_changeset);
    assert_eq!(state_root, acct_harness.original_root(), "state root should match reference");

    // --- Finalize: take_updates ---
    let acct_updates = acct_trie.take_updates();
    let a1_updates = a1_trie.take_updates();
    let a2_updates = a2_trie.take_updates();

    // --- Reuse: commit + prune ---
    acct_trie.commit_updates(&acct_updates.updated_nodes, &acct_updates.removed_nodes);
    a1_trie.commit_updates(&a1_updates.updated_nodes, &a1_updates.removed_nodes);
    a2_trie.commit_updates(&a2_updates.updated_nodes, &a2_updates.removed_nodes);

    // Post-commit root should still be state_root.
    let post_commit_root = acct_trie.root();
    assert_eq!(post_commit_root, state_root, "root should be unchanged after commit_updates");

    // Prune account trie, retaining only A1 and A2 paths.
    let retained: Vec<Nibbles> = acct_keys[0..2].iter().map(|k| Nibbles::unpack(*k)).collect();
    acct_trie.prune(&retained);

    // Post-prune root should still be state_root.
    let post_prune_root = acct_trie.root();
    assert_eq!(post_prune_root, state_root, "root should be unchanged after prune");
}

/// Prewarm via `Touched`, then mutate via `Changed`.
///
/// `Touched` is used to prewarm accounts/slots before actual state changes arrive.
/// When the real `Changed` update arrives, it overwrites the `Touched` entry.
/// This test verifies that prewarming followed by mutation works correctly.
pub(super) fn test_touched_prewarm_then_changed_update<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);
    let key4 = B256::with_last_byte(0x40);
    let key5 = B256::with_last_byte(0x50);

    let base_storage: BTreeMap<B256, U256> = [
        (key1, U256::from(1)),
        (key2, U256::from(2)),
        (key3, U256::from(3)),
        (key4, U256::from(4)),
        (key5, U256::from(5)),
    ]
    .into_iter()
    .collect();

    let mut harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Step 1: Prewarm 3 keys with Touched — all should be drained (paths are revealed).
    let mut leaf_updates: B256Map<LeafUpdate> =
        [(key1, LeafUpdate::Touched), (key2, LeafUpdate::Touched), (key3, LeafUpdate::Touched)]
            .into_iter()
            .collect();

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves with Touched should succeed");

    assert!(targets.is_empty(), "no callback should fire for Touched on fully revealed paths");
    assert!(leaf_updates.is_empty(), "all Touched keys should be drained");

    // Step 2: Now send Changed for 2 of the touched keys + 1 new key.
    let new_key = B256::with_last_byte(0x60);
    let mut changeset: BTreeMap<B256, U256> = BTreeMap::new();
    changeset.insert(key1, U256::from(100)); // modify key1
    changeset.insert(key2, U256::from(200)); // modify key2
    changeset.insert(new_key, U256::from(600)); // insert new key

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // Step 3: Compute root and verify against reference.
    let root = trie.root();

    harness.apply_changeset(changeset);
    assert_eq!(
        root,
        harness.original_root(),
        "root should match reference after Touched + Changed"
    );
}

/// Touched on blinded path triggers proof callback, then Changed
/// for the same key succeeds after reveal.
///
/// A `Touched` update hits a blinded node, triggering a proof request. After the proof is
/// revealed, a `Changed` update for the same key succeeds. This is the prewarm-miss →
/// reveal → update sequence.
pub(super) fn test_touched_on_blinded_triggers_proof_then_changed_succeeds<
    T: SparseTrie + Default,
>() {
    // Two groups of 16 keys each to create blinded subtries.
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

    let mut harness = SuiteTestHarness::new(base_storage);

    // Reveal only group A — group B's subtrie is blinded.
    let mut trie: T = harness.init_trie_with_targets(&group_a_keys, false);

    // Step 1: Touched on a key in group B's blinded subtrie → callback fires.
    let target_key = group_b_keys[0];
    let mut leaf_updates: B256Map<LeafUpdate> = once((target_key, LeafUpdate::Touched)).collect();

    let mut targets: Vec<ProofV2Target> = Vec::new();
    trie.update_leaves(&mut leaf_updates, |key, min_len| {
        targets.push(ProofV2Target::new(key).with_min_len(min_len));
    })
    .expect("update_leaves with Touched should succeed");

    assert!(!targets.is_empty(), "callback should fire for Touched on blinded path");
    assert!(!leaf_updates.is_empty(), "Touched key should remain in map when blinded");

    // Step 2: Reveal the proof for the requested targets.
    let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof_nodes).expect("reveal should succeed");

    // Step 3: Replace Touched with Changed(new_value) in the map.
    let new_value = U256::from(999);
    leaf_updates.insert(target_key, LeafUpdate::Changed(encode_fixed_size(&new_value).to_vec()));

    // Step 4: update_leaves again — key should now be drained.
    trie.update_leaves(&mut leaf_updates, |_, _| {})
        .expect("update_leaves with Changed should succeed");

    assert!(leaf_updates.is_empty(), "Changed key should be drained after reveal");

    // Step 5: Compute root and verify against reference.
    let root = trie.root();

    let mut changeset = BTreeMap::new();
    changeset.insert(target_key, new_value);
    harness.apply_changeset(changeset);
    assert_eq!(
        root,
        harness.original_root(),
        "root should match reference after Touched-miss → reveal → Changed"
    );
}

/// `get_leaf_value` for storage root lookup.
///
/// Simulates the `SparseStateTrie::update_account` pattern: read existing leaf via
/// `get_leaf_value`, decode, modify (change one field while preserving another), re-encode,
/// and update. Verifies that root matches reference.
pub(super) fn test_get_leaf_value_for_storage_root_lookup<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(100)), (key2, U256::from(200)), (key3, U256::from(300))]
            .into_iter()
            .collect();

    let mut harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    // Step 1: Read existing leaf value via get_leaf_value.
    let key1_nibbles = Nibbles::unpack(key1);
    let existing_rlp = trie
        .get_leaf_value(&key1_nibbles)
        .expect("get_leaf_value should return Some for existing key")
        .clone();

    // Step 2: Decode the value and verify it matches expected.
    let decoded: U256 =
        Decodable::decode(&mut existing_rlp.as_slice()).expect("should decode as U256");
    assert_eq!(decoded, U256::from(100), "decoded value should match original");

    // Step 3: Modify the value (simulates changing balance while keeping storage root).
    let new_value = U256::from(999);
    let new_value_rlp = encode_fixed_size(&new_value).to_vec();

    // Step 4: Update the leaf with the re-encoded value.
    let mut leaf_updates: B256Map<LeafUpdate> =
        once((key1, LeafUpdate::Changed(new_value_rlp))).collect();
    trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");

    // Step 5: Compute root and verify against reference.
    let root = trie.root();

    let mut changeset = BTreeMap::new();
    changeset.insert(key1, new_value);
    harness.apply_changeset(changeset);
    assert_eq!(
        root,
        harness.original_root(),
        "root should match reference after get→decode→re-encode→update"
    );
}

/// Before updating, check existence via `find_leaf`, then
/// insert/modify, and verify `find_leaf` reflects the new state.
pub(super) fn test_find_leaf_before_update_to_check_existence<T: SparseTrie + Default>() {
    let key1 = B256::with_last_byte(0x10);
    let key2 = B256::with_last_byte(0x20);
    let key3 = B256::with_last_byte(0x30);
    let nonexistent_key = B256::with_last_byte(0x40);

    let base_storage: BTreeMap<B256, U256> =
        [(key1, U256::from(1)), (key2, U256::from(2)), (key3, U256::from(3))].into_iter().collect();

    let mut harness = SuiteTestHarness::new(base_storage);
    let mut trie: T = harness.init_trie_fully_revealed(false);

    let key2_nibbles = Nibbles::unpack(key2);
    let nonexistent_nibbles = Nibbles::unpack(nonexistent_key);

    // Step 1: find_leaf(existing_key, None) → Exists
    assert_eq!(
        trie.find_leaf(&key2_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::Exists,
        "existing key should return Exists before update"
    );

    // Step 2: find_leaf(nonexistent_key, None) → NonExistent
    assert_eq!(
        trie.find_leaf(&nonexistent_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::NonExistent,
        "nonexistent key should return NonExistent before update"
    );

    // Step 3: Modify existing key2 and insert nonexistent_key.
    let new_key2_value = U256::from(222);
    let new_insert_value = U256::from(444);
    let mut leaf_updates: B256Map<LeafUpdate> = [
        (key2, LeafUpdate::Changed(encode_fixed_size(&new_key2_value).to_vec())),
        (nonexistent_key, LeafUpdate::Changed(encode_fixed_size(&new_insert_value).to_vec())),
    ]
    .into_iter()
    .collect();
    trie.update_leaves(&mut leaf_updates, |_, _| {}).expect("update_leaves should succeed");

    // Step 4: Compute root and verify against reference.
    let root = trie.root();

    let mut changeset = BTreeMap::new();
    changeset.insert(key2, new_key2_value);
    changeset.insert(nonexistent_key, new_insert_value);
    harness.apply_changeset(changeset);
    assert_eq!(root, harness.original_root(), "root should match reference after find→update");

    // Step 5: find_leaf(nonexistent_key, None) → now Exists
    assert_eq!(
        trie.find_leaf(&nonexistent_nibbles, None).expect("find_leaf should succeed"),
        LeafLookup::Exists,
        "previously nonexistent key should return Exists after insertion"
    );
}

/// After prune, hot leaves update immediately; cold leaves need re-reveal.
///
/// Build 10-leaf trie, do Block 1 (update K1,K2,K3 → commit → prune retaining K1,K2),
/// then Block 2: update K1 (hot, works immediately), update K5 (cold, needs re-reveal).
pub(super) fn test_prune_then_reuse_for_next_block<T: SparseTrie + Default>() {
    // Build a trie with 10 leaves.
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

    // Cache initial hashes.
    let _ = trie.root();

    // --- Block 1: Update K1 (keys[0]), K2 (keys[1]), K3 (keys[2]) ---
    let mut changeset1: BTreeMap<B256, U256> = BTreeMap::new();
    changeset1.insert(keys[0], U256::from(100));
    changeset1.insert(keys[1], U256::from(200));
    changeset1.insert(keys[2], U256::from(300));
    let mut leaf_updates1 = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates1);
    let root1 = trie.root();

    harness.apply_changeset(changeset1);
    assert_eq!(root1, harness.original_root(), "block 1 root should match reference");

    let updates1 = trie.take_updates();
    trie.commit_updates(&updates1.updated_nodes, &updates1.removed_nodes);

    // --- Prune: retain only K1 and K2 as "hot" ---
    let retained: Vec<Nibbles> = [keys[0], keys[1]].iter().map(|k| Nibbles::unpack(*k)).collect();
    trie.prune(&retained);

    // --- Block 2 — hot path: Update K1 with a new value ---
    let mut changeset_hot: BTreeMap<B256, U256> = BTreeMap::new();
    changeset_hot.insert(keys[0], U256::from(999));
    let mut leaf_updates_hot = SuiteTestHarness::leaf_updates(&changeset_hot);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_hot);
    let root_hot = trie.root();

    harness.apply_changeset(changeset_hot);
    assert_eq!(
        root_hot,
        harness.original_root(),
        "hot path root should match reference (K1 updated)"
    );

    // --- Block 2 — cold path: Update K5 (keys[4], pruned) ---
    let mut changeset_cold: BTreeMap<B256, U256> = BTreeMap::new();
    changeset_cold.insert(keys[4], U256::from(555));
    let mut leaf_updates_cold = SuiteTestHarness::leaf_updates(&changeset_cold);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_cold);
    let root_cold = trie.root();

    harness.apply_changeset(changeset_cold);
    assert_eq!(
        root_cold,
        harness.original_root(),
        "cold path root should match reference (K1 and K5 updated)"
    );
}
