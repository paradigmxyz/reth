use super::*;
use reth_execution_errors::SparseTrieErrorKind;
use reth_trie_sparse::SparseTrieUpdates;

/// Branch root initializes correctly and produces correct hash.
///
/// Two leaves whose first nibbles differ produce a branch root node.
/// After `set_root` + `reveal_nodes`, `root()` must match the reference hash.
pub(super) fn test_set_root_with_branch_node<T: SparseTrie>(new_trie: fn() -> T) {
    // Keys whose first nibbles differ → branch at root.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10; // first nibble = 1
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20; // first nibble = 2
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);
    let root = trie.root(epoch(0));
    assert_eq!(root, harness.original_root());
}

/// Single-leaf root initializes correctly.
///
/// A single key-value pair produces a leaf root node. After `set_root` + `root()`,
/// the hash must match the reference trie.
pub(super) fn test_set_root_with_leaf_node<T: SparseTrie>(new_trie: fn() -> T) {
    let storage: BTreeMap<B256, U256> = BTreeMap::from([(B256::ZERO, U256::from(42))]);

    let harness = SuiteTestHarness::new(storage);
    let mut root_nodes = harness.root_nodes();
    let mut trie = (new_trie)();
    trie.set_root(&mut root_nodes, true).expect("set_root should succeed");
    let root = trie.root(epoch(0));
    assert_eq!(root, harness.original_root());
}

/// Extension root (shared prefix) initializes correctly.
///
/// Two keys sharing a long common prefix produce an extension root node.
/// After `set_root` + `reveal_nodes`, `root()` must match the reference hash.
pub(super) fn test_set_root_with_extension_node<T: SparseTrie>(new_trie: fn() -> T) {
    // Keys that share first byte 0xAB → extension root.
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0xAB;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0xAB;
    key_b.0[31] = 0x01;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::from(100)), (key_b, U256::from(200))]);

    let harness = SuiteTestHarness::new(storage);
    let mut trie = (new_trie)();
    let root = harness.root_node();
    let TrieNode::Extension(extension) = &root.node else {
        panic!("fixture root must be an extension");
    };
    let error = trie.set_root(&mut [root.clone()], true).unwrap_err();
    match error.kind() {
        SparseTrieErrorKind::Reveal { path, node } => {
            assert_eq!(*path, Nibbles::default());
            assert_eq!(format!("{node:?}"), format!("{extension:?}"));
        }
        other => panic!("expected Reveal error, got {other:?}"),
    }
    assert_eq!(trie.take_updates(), SparseTrieUpdates::default());

    // Rejection must preserve the prior retention policy, not the requested opposite policy.
    let storage = BTreeMap::from([
        (B256::right_padding_from(&[0x12, 0x31]), U256::ONE),
        (B256::right_padding_from(&[0x12, 0x32]), U256::from(2)),
        (B256::right_padding_from(&[0x12, 0x31, 0x10]), U256::from(3)),
    ]);
    let probe_harness = SuiteTestHarness::new(storage);
    for retain_updates in [false, true] {
        let mut probe = (new_trie)();
        probe.set_updates(retain_updates);
        let probe_error = probe.set_root(&mut [root.clone()], !retain_updates).unwrap_err();
        assert_eq!(probe_error.to_string(), error.to_string());
        // No intervening successful set_root may reset the policy being checked.
        let mut updates = SuiteTestHarness::leaf_updates(probe_harness.storage());
        probe
            .update_leaves(&mut updates, |_, _| {
                panic!("insertion into an empty trie needs no proof")
            })
            .unwrap();
        assert!(updates.is_empty());
        assert_eq!(probe.root(epoch(1)), probe_harness.original_root());
        let updates = probe.take_updates();
        assert_eq!(updates.updated_nodes.is_empty(), !retain_updates);
        assert!(updates.removed_nodes.is_empty());
    }

    // A rejected incomplete root must not prevent initialization with the full calculator pair.
    let mut nodes = harness.root_nodes();
    trie.set_root(&mut nodes, true).expect("complete root pair should succeed after rejection");
    trie.reveal_nodes(&mut nodes).unwrap();
    assert_eq!(trie.root(epoch(0)), harness.original_root());
    assert_eq!(trie.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));
    assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
}

/// `retain_updates=true` enables update tracking.
///
/// When `set_root` is called with `retain_updates = true`, subsequent mutations
/// should be tracked and `take_updates()` should return non-empty results.
pub(super) fn test_set_root_retains_updates_when_requested<T: SparseTrie>(new_trie: fn() -> T) {
    // Build a trie with enough leaves to produce non-root branch nodes with hash children.
    // We need leaves sharing a prefix nibble so that intermediate branch nodes are created,
    // and enough entries that children are hashed (RLP ≥ 32 bytes).
    let mut storage: BTreeMap<B256, U256> = BTreeMap::new();
    for i in 0u8..16 {
        let mut key = B256::ZERO;
        key.0[0] = 0x10; // shared first nibble → branch at nibble 1
        key.0[1] = i * 16; // differ in second nibble → branch below
        storage.insert(key, U256::from(i as u64 + 1));
    }

    let harness = SuiteTestHarness::new(storage);
    let mut trie: T = harness.init_trie_fully_revealed(true, new_trie);

    // Compute root once so branch hashes are cached.
    let _ = trie.root(epoch(0));

    // Add a new leaf under the same prefix so non-root branch nodes change.
    let mut new_key = B256::ZERO;
    new_key.0[0] = 0x10;
    new_key.0[1] = 0xFF;
    let changeset: BTreeMap<B256, U256> = BTreeMap::from([(new_key, U256::from(100))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    // Compute root to finalize hashes and generate update actions.
    let _ = trie.root(epoch(0));

    // take_updates should return non-empty updates.
    let updates = trie.take_updates();
    assert!(
        !updates.updated_nodes.is_empty() || !updates.removed_nodes.is_empty(),
        "take_updates should be non-empty when retain_updates=true: updated={}, removed={}",
        updates.updated_nodes.len(),
        updates.removed_nodes.len(),
    );
}

/// `retain_updates=false` disables update tracking.
///
/// When `set_root` is called with `retain_updates = false`, `take_updates()` should
/// return an empty `SparseTrieUpdates` even after mutations.
pub(super) fn test_set_root_does_not_retain_updates_when_not_requested<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let mut key_a = B256::ZERO;
    key_a.0[0] = 0x10;
    let mut key_b = B256::ZERO;
    key_b.0[0] = 0x20;
    let mut key_c = B256::ZERO;
    key_c.0[0] = 0x30;
    let storage: BTreeMap<B256, U256> =
        BTreeMap::from([(key_a, U256::ONE), (key_b, U256::from(2)), (key_c, U256::from(3))]);

    let harness = SuiteTestHarness::new(storage);
    // retain_updates = false
    let mut trie: T = harness.init_trie_fully_revealed(false, new_trie);

    // Modify a leaf.
    let changeset: BTreeMap<B256, U256> = BTreeMap::from([(key_a, U256::from(99))]);
    let mut leaf_updates = SuiteTestHarness::leaf_updates(&changeset);
    harness.reveal_and_update(&mut trie, &mut leaf_updates);

    let _ = trie.root(epoch(0));

    let updates = trie.take_updates();
    assert!(
        updates.updated_nodes.is_empty() && updates.removed_nodes.is_empty(),
        "take_updates should be empty when retain_updates=false: updated={}, removed={}",
        updates.updated_nodes.len(),
        updates.removed_nodes.len(),
    );
}

/// `EmptyRoot` produces `EMPTY_ROOT_HASH`.
///
/// Setting the root to `TrieNode::EmptyRoot` should leave the trie in its initial
/// empty state, returning `EMPTY_ROOT_HASH` from `root()`.
pub(super) fn test_set_root_with_empty_root<T: SparseTrie>(new_trie: fn() -> T) {
    let mut trie = (new_trie)();
    trie.set_root(&mut [ProofTrieNode::empty()], true).expect("set_root should succeed");
    assert_eq!(trie.root(epoch(0)), EMPTY_ROOT_HASH);
}
