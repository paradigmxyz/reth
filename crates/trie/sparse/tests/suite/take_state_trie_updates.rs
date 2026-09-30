//! Complete-node variants of the compact update lifecycle tests.
use super::*;

pub(super) fn test_take_state_trie_updates_returns_empty_when_not_tracking<T: SparseTrie>(
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

    let updates = take_state_updates(&mut trie);
    assert!(updates.0.is_empty(), "updated_nodes should be empty when not tracking");
    assert!(updates.1.is_empty(), "removed_nodes should be empty when not tracking");
}

pub(super) fn test_take_state_trie_updates_resets_after_take<T: SparseTrie>(new_trie: fn() -> T) {
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10;
        key.0[1] = i * 16;
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    trie.set_state_trie_updates(true);

    let _ = trie.root(epoch(0));

    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    key_a.0[1] = 0xFF;
    let changeset_a: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::from(999))]);
    let mut leaf_updates_a = SuiteTestHarness::leaf_updates(&changeset_a);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_a);
    let _ = trie.root(epoch(0));
    let updates1 = take_state_updates(&mut trie);

    assert!(
        !updates1.0.is_empty() || !updates1.1.is_empty(),
        "updates1 should be non-empty after adding leaf A",
    );

    let updates_empty = take_state_updates(&mut trie);
    assert!(
        updates_empty.0.is_empty() && updates_empty.1.is_empty(),
        "take_updates right after a take should be empty (accumulator was reset)",
    );

    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x10;
    key_b.0[1] = 0xFE;
    let changeset_b: BTreeMap<B256, U256> = BTreeMap::from([(key_b, U256::from(888))]);
    let mut leaf_updates_b = SuiteTestHarness::leaf_updates(&changeset_b);
    harness.reveal_and_update(&mut trie, &mut leaf_updates_b);
    let _ = trie.root(epoch(0));
    let updates2 = take_state_updates(&mut trie);

    assert!(
        !updates2.0.is_empty() || !updates2.1.is_empty(),
        "updates2 should be non-empty after adding leaf B",
    );
}

pub(super) fn test_take_state_trie_updates_contains_updated_and_removed_nodes<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    let mut val = 1u64;
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
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    trie.set_state_trie_updates(true);

    let _ = trie.root(epoch(0));

    let _ = take_state_updates(&mut trie);

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
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    new_key.0[2] = 0xFF;
    changeset.insert(new_key, U256::from(9999));

    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root(epoch(0));
    let updates = take_state_updates(&mut trie);

    assert!(
        updates.0.contains_key(&Nibbles::from_nibbles([0x1, 0x0])),
        "branch [1,0] should be in updated_nodes after adding a leaf in group 0x1"
    );

    assert!(
        updates.1.contains(&Nibbles::from_nibbles([0x2, 0x0])),
        "[2,0] was a real DB node and should appear in removed_nodes"
    );

    for nibble in 0u8..16 {
        let sub_path = Nibbles::from_nibbles([0x2, 0x0, nibble]);
        assert!(
            updates.1.contains(&sub_path),
            "[2,0,{nibble:x}] was a real DB node and should appear in removed_nodes"
        );
    }

    for path in &updates.1 {
        assert!(
            !updates.0.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes"
        );
    }
}

pub(super) fn test_take_state_trie_updates_cross_cancellation_across_root_calls<T: SparseTrie>(
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
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    trie.set_state_trie_updates(true);

    let _ = trie.root(epoch(0));

    let changeset1: BTreeMap<B256, U256> =
        [(key_new_a, val), (key_new_b, val)].into_iter().collect();
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset1);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);
    let _ = trie.root(epoch(0));

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

    let updates = take_state_updates(&mut trie);

    for path in &updates.1 {
        assert!(
            !updates.0.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes \
             (cross-cancellation bug)"
        );
    }
}

pub(super) fn test_take_state_trie_updates_no_duplicate_updated_and_removed_nodes<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x00;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x01;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x02;

    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(1)), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);
    trie.set_state_trie_updates(true);

    let _ = trie.root(epoch(0));

    let mut remove_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    remove_changeset.insert(key_c, U256::ZERO);
    let mut remove_updates = SuiteTestHarness::leaf_updates(&remove_changeset);
    harness.reveal_and_update(&mut trie, &mut remove_updates);

    let mut key_d = B256::ZERO;
    key_d.0[0] = 0x03;
    let mut insert_changeset: BTreeMap<B256, U256> = BTreeMap::new();
    insert_changeset.insert(key_d, U256::from(4));
    let mut insert_updates = SuiteTestHarness::leaf_updates(&insert_changeset);
    harness.reveal_and_update(&mut trie, &mut insert_updates);

    let _ = trie.root(epoch(0));
    let updates = take_state_updates(&mut trie);

    for path in &updates.1 {
        assert!(
            !updates.0.contains_key(path),
            "path {path:?} appears in both updated_nodes and removed_nodes"
        );
    }
}

type RawStateTrieNode = reth_trie_common::StateTrieNode<smallvec::SmallVec<[u8; 16]>>;

fn take_state_updates<T: SparseTrie>(
    trie: &mut T,
) -> (BTreeMap<Nibbles, RawStateTrieNode>, std::collections::BTreeSet<Nibbles>) {
    let updates = trie.take_state_trie_updates();
    assert!(updates.windows(2).all(|w| w[0].0 < w[1].0));
    let mut inserted = BTreeMap::new();
    let mut removed = std::collections::BTreeSet::new();
    for (p, n) in updates {
        if let Some(n) = n {
            inserted.insert(p, n);
        } else {
            removed.insert(p);
        }
    }
    (inserted, removed)
}
