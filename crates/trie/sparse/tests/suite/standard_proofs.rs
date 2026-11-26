//! Calculator-produced extension/branch pairs preserve roots and persistence masks.

use super::*;
use reth_trie_common::TrieMask;
use reth_trie_sparse::SparseTrieUpdates;

pub(super) fn root_extension_branch_pair_preserves_updates<T: SparseTrie>(new_trie: fn() -> T) {
    extension_branch_pair_preserves_updates(new_trie, false, false);
}

pub(super) fn nonroot_extension_branch_pair_preserves_updates<T: SparseTrie>(new_trie: fn() -> T) {
    extension_branch_pair_preserves_updates(new_trie, true, false);
}

pub(super) fn duplicate_calculator_extension_pairs_preserve_updates<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    for nonroot in [false, true] {
        extension_branch_pair_preserves_updates(new_trie, nonroot, true);
    }
}

fn extension_branch_pair_preserves_updates<T: SparseTrie>(
    new_trie: fn() -> T,
    nonroot: bool,
    duplicate: bool,
) {
    let mut storage = BTreeMap::from([
        (B256::right_padding_from(&[0x12, 0x31]), U256::ONE),
        (B256::right_padding_from(&[0x12, 0x32]), U256::from(2)),
    ]);
    storage.insert(B256::right_padding_from(&[0x12, 0x31, 0x10]), U256::from(5));
    if nonroot {
        storage.insert(B256::right_padding_from(&[0x90]), U256::from(4));
    }
    let harness = SuiteTestHarness::new(storage);
    let key = B256::right_padding_from(&[0x12, 0x40]);
    let mut partial = (new_trie)();
    let mut nodes = if nonroot {
        partial.set_root(&mut harness.root_nodes(), true).unwrap();
        let (mut proof, _) = harness.proof_v2(&mut [ProofV2Target::new(key)]);
        proof.retain(|node| !node.path.is_empty());
        proof
    } else {
        harness.root_nodes()
    };

    // An absence target diverging inside the extension still retains its separate branch.
    assert_eq!(nodes.len(), 2);
    let branch = nodes.iter().find(|node| matches!(node.node, TrieNode::Branch(_))).unwrap();
    assert_eq!(branch.path, Nibbles::from_nibbles([1, 2, 3]));
    assert!(branch.masks.is_some(), "the child branch has persisted descendants");
    let extension = nodes.iter().find(|node| matches!(node.node, TrieNode::Extension(_))).unwrap();
    let TrieNode::Extension(extension_node) = &extension.node else { unreachable!() };
    assert!(extension_node.child.is_hash());
    assert_eq!(extension.path, if nonroot { Nibbles::from_nibbles([1]) } else { Nibbles::new() });
    assert_eq!(
        extension_node.key,
        if nonroot { Nibbles::from_nibbles([2, 3]) } else { Nibbles::from_nibbles([1, 2, 3]) }
    );
    if duplicate {
        // Separate workers can append identical calculator batches without deduplication.
        let mut overlapping = nodes.clone();
        for node in &mut overlapping {
            node.masks = None;
        }
        nodes.extend(overlapping);
    }
    if !nonroot {
        partial.set_root(&mut nodes, true).unwrap();
    }
    // Root initialization consumes its pair; reusing the batch must remain harmless.
    partial.reveal_nodes(&mut nodes).unwrap();
    assert_eq!(partial.root(epoch(0)), harness.original_root());
    let mut revealed = harness.init_trie_fully_revealed(true, new_trie);
    assert_eq!(revealed.root(epoch(0)), harness.original_root());
    assert_eq!(partial.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));
    assert_eq!(partial.take_updates(), SparseTrieUpdates::default());
    assert_eq!(revealed.take_updates(), SparseTrieUpdates::default());

    let changes = BTreeMap::from([(key, U256::from(3))]);
    let mut partial_updates = SuiteTestHarness::leaf_updates(&changes);
    partial
        .update_leaves(&mut partial_updates, |_, _| panic!("the branch masks are already revealed"))
        .unwrap();
    assert!(partial_updates.is_empty());
    let mut revealed_updates = SuiteTestHarness::leaf_updates(&changes);
    harness.reveal_and_update(&mut revealed, &mut revealed_updates);
    assert!(revealed_updates.is_empty());
    assert_eq!(partial.root(epoch(1)), harness.get_root_with_updates(&changes).0);
    assert_eq!(partial.root(epoch(1)), revealed.root(epoch(1)));
    let updates = partial.take_updates();
    assert_eq!(updates, revealed.take_updates());
    let parent = &updates.updated_nodes[&Nibbles::from_nibbles([1, 2])];
    assert_eq!(parent.hash_mask, TrieMask::from_nibble(3));
    assert_eq!(parent.tree_mask, TrieMask::from_nibble(3));
}
