//! Regressions for standard extensions whose branch children have not been revealed.

use super::*;
use reth_trie_common::TrieMask;

pub(super) fn collapse_onto_an_unrevealed_extension_child<T: SparseTrie>(new_trie: fn() -> T) {
    let harness = extension_harness();
    let mut trie = harness.init_trie_with_targets(&[], false, new_trie);
    let key = B256::right_padding_from(&[0x12, 0x40]);
    let mut inserts = SuiteTestHarness::leaf_updates(&[(key, U256::from(3))].into());
    trie.update_leaves(&mut inserts, |_, _| panic!("divergent insertion needs no child proof"))
        .unwrap();
    assert!(inserts.is_empty());

    // The new branch's surviving child is the original, still-opaque branch at [1, 2, 3].
    let mut removals = SuiteTestHarness::leaf_updates(&[(key, U256::ZERO)].into());
    trie.update_leaves(&mut removals, |_, _| panic!("opaque branch can survive the collapse"))
        .unwrap();
    assert!(removals.is_empty());
    assert_eq!(trie.root(epoch(1)), harness.original_root());
}

pub(super) fn hash_a_split_extension_with_an_unrevealed_branch_child<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let harness = extension_harness();
    let mut trie = harness.init_trie_with_targets(&[], false, new_trie);
    let key = B256::right_padding_from(&[0x14]);
    let mut inserts = SuiteTestHarness::leaf_updates(&[(key, U256::from(3))].into());
    trie.update_leaves(&mut inserts, |_, _| panic!("divergent insertion needs no child proof"))
        .unwrap();
    assert!(inserts.is_empty());

    // Splitting at [1] retains an extension [3] above the opaque branch at [1, 2, 3].
    let mut expected_storage = harness.storage().clone();
    expected_storage.insert(key, U256::from(3));
    let expected = SuiteTestHarness::new(expected_storage);
    assert_eq!(trie.root(epoch(1)), expected.original_root());
}

pub(super) fn divergent_extension_insert_fetches_unknown_branch_masks<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let harness = extension_harness();
    let mut partial = harness.init_trie_with_targets(&[], true, new_trie);
    let mut revealed = harness.init_trie_fully_revealed(true, new_trie);
    let key = B256::right_padding_from(&[0x12, 0x40]);
    let changes = [(key, U256::from(3))].into();
    let mut partial_updates = SuiteTestHarness::leaf_updates(&changes);
    let mut requests = Vec::new();
    partial
        .update_leaves(&mut partial_updates, |key, parent| requests.push((key, parent)))
        .unwrap();
    assert_eq!(requests.len(), 1, "an opaque hash cannot establish its branch's tree mask");
    assert_eq!(partial_updates.len(), 1, "the insert must wait for the branch masks");
    harness.reveal_and_update(&mut partial, &mut partial_updates);
    assert!(partial_updates.is_empty());

    let mut revealed_updates = SuiteTestHarness::leaf_updates(&changes);
    harness.reveal_and_update(&mut revealed, &mut revealed_updates);
    assert!(revealed_updates.is_empty());
    assert_eq!(partial.root(epoch(1)), revealed.root(epoch(1)));
    let updates = partial.take_updates();
    assert_eq!(updates, revealed.take_updates());
    let parent = &updates.updated_nodes[&Nibbles::from_nibbles([1, 2])];
    assert_eq!(parent.hash_mask, TrieMask::from_nibble(3));
    assert!(parent.tree_mask.is_empty(), "the old branch has no persisted descendants");
}

pub(super) fn split_opaque_extension_at_subtrie_boundary<T: SparseTrie>(new_trie: fn() -> T) {
    let first = B256::right_padding_from(&[0x12, 0x10]);
    let second = B256::right_padding_from(&[0x12, 0x20]);
    let harness = SuiteTestHarness::new([(first, U256::ONE), (second, U256::from(2))].into());
    let TrieNode::Extension(extension) = harness.root_node().node else {
        panic!("fixture root must be an extension")
    };
    assert_eq!(extension.key, Nibbles::from_nibbles([1, 2]));
    assert!(extension.child.is_hash());
    let mut trie = harness.init_trie_with_targets(&[], false, new_trie);
    let inserted = B256::right_padding_from(&[0x13]);
    let mut inserts = SuiteTestHarness::leaf_updates(&[(inserted, U256::from(3))].into());
    // Splitting removes the old extension and leaves its opaque branch at the depth-2 boundary.
    trie.update_leaves(&mut inserts, |_, _| panic!("divergent insertion needs no child proof"))
        .unwrap();
    assert!(inserts.is_empty());
    let mut expected_storage = harness.storage().clone();
    expected_storage.insert(inserted, U256::from(3));
    let expected = SuiteTestHarness::new(expected_storage);
    assert_eq!(trie.root(epoch(1)), expected.original_root());

    // Collapse onto the opaque subtrie, then split it back out before revealing its branch.
    let mut removals = SuiteTestHarness::leaf_updates(&[(inserted, U256::ZERO)].into());
    trie.update_leaves(&mut removals, |_, _| panic!("opaque subtrie can survive the collapse"))
        .unwrap();
    assert!(removals.is_empty());
    assert_eq!(trie.root(epoch(2)), harness.original_root());
    let mut inserts = SuiteTestHarness::leaf_updates(&[(inserted, U256::from(3))].into());
    trie.update_leaves(&mut inserts, |_, _| panic!("divergent insertion needs no child proof"))
        .unwrap();
    assert!(inserts.is_empty());
    assert_eq!(trie.root(epoch(3)), expected.original_root());

    // Reveal and modify the standalone branch after it has become a subtrie root.
    let changes = [(first, U256::from(4))].into();
    let mut updates = SuiteTestHarness::leaf_updates(&changes);
    expected.reveal_and_update(&mut trie, &mut updates);
    assert!(updates.is_empty());
    assert_eq!(trie.root(epoch(4)), expected.get_root_with_updates(&changes).0);
    assert_eq!(trie.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
}

fn extension_harness() -> SuiteTestHarness {
    let harness = SuiteTestHarness::new(
        [
            (B256::right_padding_from(&[0x12, 0x31]), U256::ONE),
            (B256::right_padding_from(&[0x12, 0x32]), U256::from(2)),
        ]
        .into(),
    );
    let TrieNode::Extension(extension) = harness.root_node().node else {
        panic!("fixture root must be a standard extension")
    };
    assert_eq!(extension.key, Nibbles::from_nibbles([1, 2, 3]));
    harness
}
