//! Partial extension proofs and calculator-produced pairs preserve roots and persistence masks.

use super::*;
use reth_trie_common::{ProofV2TargetParent, TrieMask};
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

/// Extension-only proofs preserve exclusion knowledge and hashes at every arena boundary.
pub(super) fn test_standalone_extensions<T: SparseTrie>(new_trie: fn() -> T) {
    for depth in 0..=4 {
        for retain_updates in [false, true] {
            let first = B256::right_padding_from(&[0x12, 0x34, 0x56]);
            let second = B256::right_padding_from(&[0x12, 0x34, 0x57]);
            let third = B256::right_padding_from(&[0x12, 0x34, 0x56, 0x10]);
            let prefix = Nibbles::from_nibbles([1, 2, 3, 4, 5]);
            let mut storage = BTreeMap::from([
                (first, U256::ONE),
                (second, U256::from(2)),
                (third, U256::from(3)),
            ]);
            for ancestor in 0..depth {
                let mut path = prefix.slice(..ancestor);
                path.push(9);
                storage.insert(B256::right_padding_from(&path.pack()), U256::from(4));
            }
            let harness = SuiteTestHarness::new(storage);
            let mut trie = (new_trie)();
            let (full, _) = harness.proof_v2(
                &mut harness.storage().keys().copied().map(ProofV2Target::new).collect::<Vec<_>>(),
            );
            let extension = full.iter().find(|node| node.path == prefix.slice(..depth)).unwrap();
            let TrieNode::Extension(extension_node) = &extension.node else {
                panic!("expected extension at depth {depth}")
            };
            let branch_hash = extension_node.child.as_hash().unwrap();
            let mut partial = full
                .iter()
                .filter(|node| !node.path.starts_with(&prefix))
                .cloned()
                .collect::<Vec<_>>();
            trie.set_root(&mut partial, retain_updates).unwrap();
            trie.reveal_nodes(&mut partial).unwrap();
            assert_eq!(trie.root(epoch(0)), harness.original_root());
            assert_eq!(trie.root(epoch(0)), harness.original_root());
            assert_eq!(trie.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));
            assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
            assert_eq!(trie.get_leaf_value(&Nibbles::unpack(first)), None);
            assert_eq!(
                trie.find_leaf(&Nibbles::unpack(first), None),
                Err(LeafLookupError::BlindedNode { path: prefix, hash: branch_hash })
            );

            let mut exclusion = prefix.slice(..depth);
            exclusion.push(8);
            let exclusion = B256::right_padding_from(&exclusion.pack());
            assert_eq!(
                trie.find_leaf(&Nibbles::unpack(exclusion), None),
                Ok(LeafLookup::NonExistent)
            );
            let mut touches = B256Map::from_iter([(exclusion, LeafUpdate::Touched)]);
            trie.update_leaves(&mut touches, |_, _| panic!("extension proves exclusion")).unwrap();
            assert!(touches.is_empty());
            let mut removals = B256Map::from_iter([(exclusion, LeafUpdate::Changed(Vec::new()))]);
            trie.update_leaves(&mut removals, |_, _| panic!("absent removal needs no proof"))
                .unwrap();
            assert!(removals.is_empty());
            for update in [
                LeafUpdate::Touched,
                LeafUpdate::Changed(vec![10]),
                LeafUpdate::Changed(Vec::new()),
            ] {
                let mut pending = B256Map::from_iter([(first, update)]);
                let mut requested = Vec::new();
                trie.update_leaves(&mut pending, |key, parent| requested.push((key, parent)))
                    .unwrap();
                assert_eq!(
                    requested,
                    vec![(
                        first,
                        depth
                            .checked_sub(1)
                            .map_or(ProofV2TargetParent::NONE, ProofV2TargetParent::new)
                    )]
                );
                assert_eq!(pending.len(), 1);
            }

            // Updating an unrelated sibling must preserve unknown persistence masks.
            if depth > 0 {
                let sibling = B256::right_padding_from(&[0x90]);
                let changes = BTreeMap::from([(sibling, U256::from(99))]);
                let mut reference = harness.init_trie_fully_revealed(retain_updates, new_trie);
                harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&changes));
                harness.reveal_and_update(
                    &mut reference,
                    &mut SuiteTestHarness::leaf_updates(&changes),
                );
                assert_eq!(trie.root(epoch(1)), reference.root(epoch(1)));
                assert_eq!(trie.take_updates(), reference.take_updates());
            }

            // Replaying an extension together with its branch must not swallow the later child.
            let mut branch_pair = full
                .iter()
                .filter(|node| {
                    node.path == prefix || (depth % 2 != 0 && node.path == extension.path)
                })
                .cloned()
                .collect::<Vec<_>>();
            let root_epoch = trie.root_epoch();
            let root_cached = trie.is_root_cached();
            let mut overlapping = branch_pair.clone();
            for node in &mut overlapping {
                node.masks = None;
            }
            branch_pair.extend(overlapping);
            trie.reveal_nodes(&mut branch_pair).unwrap();
            assert_eq!(trie.root_epoch(), root_epoch);
            assert_eq!(trie.is_root_cached(), root_cached);
            assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
            let mut expected = BTreeMap::new();
            if depth > 0 {
                expected.insert(B256::right_padding_from(&[0x90]), U256::from(99));
            }
            assert_eq!(trie.root(epoch(1)), harness.get_root_with_updates(&expected).0);
            assert_eq!(
                trie.find_leaf(&Nibbles::unpack(first), None),
                Err(LeafLookupError::BlindedNode {
                    path: Nibbles::from_nibbles([1, 2, 3, 4, 5, 6]),
                    hash: match &full.iter().find(|node| node.path == prefix).unwrap().node {
                        TrieNode::Branch(branch) => branch.stack[0].as_hash().unwrap(),
                        _ => unreachable!(),
                    },
                })
            );
            let mut rest = full.clone();
            trie.reveal_nodes(&mut rest).unwrap();
            assert_eq!(trie.root_epoch(), root_epoch);
            assert_eq!(trie.is_root_cached(), root_cached);
            assert_eq!(trie.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
            let changes = BTreeMap::from([(first, U256::from(11)), (second, U256::ZERO)]);
            let mut updates = SuiteTestHarness::leaf_updates(&changes);
            harness.reveal_and_update(&mut trie, &mut updates);
            assert!(updates.is_empty());
            assert_eq!(trie.root_epoch(), None);
            assert!(!trie.is_root_cached());
            // The unrelated sibling change above is part of the reference state too.
            let mut expected = changes;
            if depth > 0 {
                expected.insert(B256::right_padding_from(&[0x90]), U256::from(99));
            }
            assert_eq!(trie.root(epoch(2)), harness.get_root_with_updates(&expected).0);
            assert_eq!(trie.root_epoch(), Some(epoch(2)));
            let updates = trie.take_updates();
            // Stale overlapping proof records cannot replace modified extension state.
            trie.reveal_nodes(&mut full.clone()).unwrap();
            assert_eq!(trie.root_epoch(), Some(epoch(2)));
            assert!(trie.is_root_cached());
            assert_eq!(trie.root(epoch(3)), harness.get_root_with_updates(&expected).0);
            assert_eq!(trie.root_epoch(), Some(epoch(2)));
            assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
            if !retain_updates {
                assert_eq!(updates, SparseTrieUpdates::default());
            }
            trie.prune(epoch(3));
            assert_eq!(trie.root(epoch(3)), harness.get_root_with_updates(&expected).0);
        }
    }
}

/// Splitting and collapsing request missing masks without changing the trie before retry.
pub(super) fn test_standalone_extension_split_and_collapse<T: SparseTrie>(new_trie: fn() -> T) {
    for nonroot in [false, true] {
        let first = B256::right_padding_from(&[0x12, 0x31]);
        let second = B256::right_padding_from(&[0x12, 0x32]);
        let sibling = B256::right_padding_from(&[0x90]);
        let mut storage = BTreeMap::from([(first, U256::ONE), (second, U256::from(2))]);
        if nonroot {
            storage.insert(sibling, U256::from(3));
        }
        let harness = SuiteTestHarness::new(storage);
        for insert in [B256::right_padding_from(&[0x14]), B256::right_padding_from(&[0x12, 0x40])] {
            let mut trie = (new_trie)();
            let (mut nodes, _) =
                harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(sibling)]);
            nodes.retain(|node| node.path.len() < 3);
            trie.set_root(&mut nodes, true).unwrap();
            trie.reveal_nodes(&mut nodes).unwrap();
            assert_eq!(trie.root(epoch(0)), harness.original_root());
            // Prune a separate initialized trie while its extension is still opaque.
            trie.prune(epoch(0));
            assert_eq!(trie.root(epoch(0)), harness.original_root());
            let changes = BTreeMap::from([(insert, U256::from(9))]);
            let mut updates = SuiteTestHarness::leaf_updates(&changes);
            let mut targets = Vec::new();
            trie.update_leaves(&mut updates, |key, parent| {
                targets.push(ProofV2Target::new(key).with_parent(parent))
            })
            .unwrap();
            assert_eq!(updates.len(), 1);
            assert_eq!(targets.len(), 1);
            assert_eq!(trie.root(epoch(1)), harness.original_root());
            let (mut proof, _) = harness.proof_v2(&mut targets);
            trie.reveal_nodes(&mut proof).unwrap();
            harness.reveal_and_update(&mut trie, &mut updates);
            let mut reference = harness.init_trie_fully_revealed(true, new_trie);
            harness
                .reveal_and_update(&mut reference, &mut SuiteTestHarness::leaf_updates(&changes));
            assert_eq!(trie.root(epoch(1)), reference.root(epoch(1)));
            assert_eq!(trie.take_updates(), reference.take_updates());
        }
        if nonroot {
            let mut trie = (new_trie)();
            let (mut nodes, _) =
                harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(sibling)]);
            nodes.retain(|node| node.path.len() < 3);
            trie.set_root(&mut nodes, true).unwrap();
            trie.reveal_nodes(&mut nodes).unwrap();
            let changes = BTreeMap::from([(sibling, U256::ZERO)]);
            let mut updates = SuiteTestHarness::leaf_updates(&changes);
            harness.reveal_and_update(&mut trie, &mut updates);
            let mut reference = harness.init_trie_fully_revealed(true, new_trie);
            harness
                .reveal_and_update(&mut reference, &mut SuiteTestHarness::leaf_updates(&changes));
            assert_eq!(trie.root(epoch(1)), reference.root(epoch(1)));
            assert_eq!(trie.take_updates(), reference.take_updates());
        }
    }
}

/// Embedded branch references can remain opaque just like hashed references.
pub(super) fn test_standalone_extension_embedded_branch<T: SparseTrie>(new_trie: fn() -> T) {
    let first = B256::ZERO;
    let mut second = first;
    second.0[31] = 1;
    let harness =
        SuiteTestHarness::new(BTreeMap::from([(first, U256::ONE), (second, U256::from(2))]));
    for cache_before_hydration in [false, true] {
        let mut trie = (new_trie)();
        let root = harness.root_node();
        let TrieNode::Extension(extension) = &root.node else { panic!("expected extension root") };
        assert!(!extension.child.is_hash());
        let child_path = extension.key;
        let hash = keccak256(extension.child.as_slice());
        trie.set_root(&mut [root], true).unwrap();
        assert_eq!(
            trie.find_leaf(&Nibbles::unpack(first), None),
            Err(LeafLookupError::BlindedNode { path: child_path, hash })
        );
        if cache_before_hydration {
            assert_eq!(trie.root(epoch(0)), harness.original_root());
        }
        assert_eq!(trie.is_root_cached(), cache_before_hydration);
        let (mut proof, _) =
            harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(second)]);
        proof.retain(|node| !node.path.is_empty());
        trie.reveal_nodes(&mut proof).unwrap();
        assert_eq!(trie.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));
        assert_eq!(trie.is_root_cached(), cache_before_hydration);
        assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
        assert_eq!(trie.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
        assert_eq!(trie.root(epoch(1)), harness.original_root());
        let changes = BTreeMap::from([(first, U256::ZERO)]);
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&changes));
        assert_eq!(trie.root(epoch(2)), harness.get_root_with_updates(&changes).0);
    }
}

/// Pruning opaque extensions permits subsequent targeted re-revelation.
pub(super) fn test_standalone_extension_prune_and_reveal<T: SparseTrie>(new_trie: fn() -> T) {
    for leave_sibling_unmodified in [false, true] {
        let first = B256::right_padding_from(&[0x12, 0x31]);
        let second = B256::right_padding_from(&[0x12, 0x32]);
        let sibling = B256::right_padding_from(&[0x90]);
        let harness = SuiteTestHarness::new(BTreeMap::from([
            (first, U256::ONE),
            (second, U256::from(2)),
            (sibling, U256::from(3)),
        ]));
        let mut trie = (new_trie)();
        let (mut proof, _) =
            harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(sibling)]);
        proof.retain(|node| node.path.len() < 3);
        trie.set_root(&mut proof, true).unwrap();
        trie.reveal_nodes(&mut proof).unwrap();
        let changes = if leave_sibling_unmodified {
            BTreeMap::new()
        } else {
            BTreeMap::from([(sibling, U256::from(4))])
        };
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&changes));
        let root = trie.root(epoch(1));
        assert!(trie.prune(epoch(1)) > 0);
        assert_eq!(trie.root(epoch(1)), root);
        let mut pending = B256Map::from_iter([(first, LeafUpdate::Touched)]);
        harness.reveal_and_update(&mut trie, &mut pending);
        assert!(pending.is_empty());
        assert_eq!(trie.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
        assert_eq!(trie.root(epoch(2)), harness.get_root_with_updates(&changes).0);
    }
}

/// Both upper and lower branch collapses must reveal an opaque surviving extension first.
pub(super) fn test_standalone_extension_collapse_across_arenas<T: SparseTrie>(new_trie: fn() -> T) {
    let prefix = Nibbles::from_nibbles([1, 2, 3, 4, 5]);
    let first = B256::right_padding_from(&[0x12, 0x34, 0x56]);
    let second = B256::right_padding_from(&[0x12, 0x34, 0x57]);
    for branch_depth in 0..=3 {
        let mut sibling_path = prefix.slice(..branch_depth);
        sibling_path.push(9);
        let sibling = B256::right_padding_from(&sibling_path.pack());
        let mut storage =
            BTreeMap::from([(first, U256::ONE), (second, U256::from(2)), (sibling, U256::from(3))]);
        for ancestor in 0..branch_depth {
            let mut path = prefix.slice(..ancestor);
            path.push(8);
            storage.insert(B256::right_padding_from(&path.pack()), U256::from(4));
        }
        let harness = SuiteTestHarness::new(storage);
        let (mut proof, _) = harness.proof_v2(
            &mut harness.storage().keys().copied().map(ProofV2Target::new).collect::<Vec<_>>(),
        );
        proof.retain(|node| node.path.len() < prefix.len());
        let mut trie = (new_trie)();
        trie.set_root(&mut proof, true).unwrap();
        trie.reveal_nodes(&mut proof).unwrap();
        assert_eq!(trie.root(epoch(0)), harness.original_root());
        let changes = BTreeMap::from([(sibling, U256::ZERO)]);
        let mut updates = SuiteTestHarness::leaf_updates(&changes);
        let mut targets = Vec::new();
        trie.update_leaves(&mut updates, |key, parent| {
            targets.push(ProofV2Target::new(key).with_parent(parent))
        })
        .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(
            targets.iter().map(|target| (target.key(), target.parent)).collect::<Vec<_>>(),
            vec![(
                B256::right_padding_from(&prefix.pack()),
                ProofV2TargetParent::new(branch_depth)
            )]
        );
        assert_eq!(trie.root(epoch(1)), harness.original_root());
        let (mut proof, _) = harness.proof_v2(&mut targets);
        trie.reveal_nodes(&mut proof).unwrap();
        harness.reveal_and_update(&mut trie, &mut updates);
        let mut reference = harness.init_trie_fully_revealed(true, new_trie);
        harness.reveal_and_update(&mut reference, &mut SuiteTestHarness::leaf_updates(&changes));
        assert_eq!(trie.root(epoch(1)), reference.root(epoch(1)));
        assert_eq!(trie.take_updates(), reference.take_updates());
    }
}

/// Hydrating a branch at the arena boundary must not rewrap the existing extension at that path.
pub(super) fn test_standalone_extension_boundary_hydration<T: SparseTrie>(new_trie: fn() -> T) {
    for nonroot in [false, true] {
        let first = B256::right_padding_from(&[0x12, 0x30]);
        let second = B256::right_padding_from(&[0x12, 0x40]);
        let mut storage = BTreeMap::from([(first, U256::ONE), (second, U256::from(2))]);
        if nonroot {
            storage.insert(B256::right_padding_from(&[0x90]), U256::from(3));
        }
        let harness = SuiteTestHarness::new(storage);
        let (full, _) =
            harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(second)]);
        let mut partial =
            full.iter().filter(|node| node.path.len() < 2).cloned().collect::<Vec<_>>();
        let mut trie = (new_trie)();
        trie.set_root(&mut partial, true).unwrap();
        trie.reveal_nodes(&mut partial).unwrap();
        assert_eq!(trie.root(epoch(0)), harness.original_root());
        let mut branch = full
            .iter()
            .filter(|node| node.path == Nibbles::from_nibbles([1, 2]))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(branch.len(), 1);
        trie.reveal_nodes(&mut branch).unwrap();
        assert_eq!(trie.root(epoch(0)), harness.original_root());
        let mut rest = full;
        trie.reveal_nodes(&mut rest).unwrap();
        let changes = BTreeMap::from([(first, U256::from(5))]);
        harness.reveal_and_update(&mut trie, &mut SuiteTestHarness::leaf_updates(&changes));
        assert_eq!(trie.root(epoch(1)), harness.get_root_with_updates(&changes).0);
    }
}

/// Hydration and structural changes in a snapshot leave its opaque source and siblings intact.
pub(super) fn test_standalone_extension_snapshot_isolation<T: SparseTrie + Clone>(
    new_trie: fn() -> T,
) {
    for depth in [0, 1, 2, 3, 4, 62] {
        for cached in [false, true] {
            let first = B256::ZERO;
            let second = B256::with_last_byte(1);
            let mut storage = BTreeMap::from([(first, U256::ONE), (second, U256::from(2))]);
            for ancestor in 0..depth {
                let mut key = [0; 32];
                key[ancestor / 2] = if ancestor % 2 == 0 { 0x90 } else { 0x09 };
                storage.insert(B256::from(key), U256::from(3));
            }
            let harness = SuiteTestHarness::new(storage);
            let (mut full, _) = harness.proof_v2(&mut [ProofV2Target::new(first)]);
            let mut partial =
                full.iter().filter(|node| node.path.len() <= depth).cloned().collect::<Vec<_>>();
            let mut source = (new_trie)();
            source.set_root(&mut partial, true).unwrap();
            source.reveal_nodes(&mut partial).unwrap();
            if cached {
                assert_eq!(source.root(epoch(1)), harness.original_root());
            }
            let mut sibling = source.snapshot();
            let mut fork = source.snapshot();
            fork.reveal_nodes(&mut full.clone()).unwrap();
            assert_eq!(fork.is_root_cached(), cached);
            assert_eq!(source.is_root_cached(), cached);
            assert_eq!(source.get_leaf_value(&Nibbles::unpack(first)), None);
            assert_eq!(sibling.get_leaf_value(&Nibbles::unpack(first)), None);
            assert_eq!(fork.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
            let changes = BTreeMap::from([(first, U256::from(9)), (second, U256::ZERO)]);
            harness.reveal_and_update(&mut fork, &mut SuiteTestHarness::leaf_updates(&changes));
            let mut reference = harness.init_trie_fully_revealed(true, new_trie);
            harness
                .reveal_and_update(&mut reference, &mut SuiteTestHarness::leaf_updates(&changes));
            assert_eq!(fork.root(epoch(2)), reference.root(epoch(2)));
            assert_eq!(fork.take_updates(), reference.take_updates());
            assert_eq!(source.root(epoch(3)), harness.original_root());
            assert_eq!(sibling.root(epoch(3)), harness.original_root());
            assert_eq!(source.take_updates(), SparseTrieUpdates::default());
            assert_eq!(sibling.take_updates(), SparseTrieUpdates::default());
            // A nested snapshot remains usable after both the source and its mutated parent drop.
            let mut survivor = fork.snapshot();
            source.clear();
            drop(source);
            drop(fork);
            sibling.reveal_nodes(&mut full).unwrap();
            assert_eq!(sibling.find_leaf(&Nibbles::unpack(first), None), Ok(LeafLookup::Exists));
            assert_eq!(sibling.root(epoch(4)), harness.original_root());
            survivor.prune(epoch(3));
            assert_eq!(survivor.root(epoch(4)), harness.get_root_with_updates(&changes).0);
        }
    }
}

/// A pending opaque subtrie cannot hide missing masks when its upper sibling is removed.
pub(super) fn test_standalone_extension_pending_subtrie_collapse<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let first = B256::right_padding_from(&[0x10, 0x12]);
    let second = B256::right_padding_from(&[0x10, 0x13]);
    let sibling = B256::right_padding_from(&[0x11]);
    let harness = SuiteTestHarness::new(BTreeMap::from([
        (first, U256::ONE),
        (second, U256::from(2)),
        (sibling, U256::from(3)),
    ]));
    let (full, _) = harness.proof_v2(&mut [ProofV2Target::new(first), ProofV2Target::new(sibling)]);
    let mut partial = full.iter().filter(|node| node.path.len() <= 2).cloned().collect::<Vec<_>>();
    let mut trie = (new_trie)();
    trie.set_root(&mut partial, true).unwrap();
    trie.reveal_nodes(&mut partial).unwrap();
    assert_eq!(trie.root(epoch(1)), harness.original_root());
    let changes = BTreeMap::from([(first, U256::from(9)), (sibling, U256::ZERO)]);
    let mut pending = SuiteTestHarness::leaf_updates(&changes);
    let mut targets = Vec::new();
    trie.update_leaves(&mut pending, |key, parent| {
        targets.push(ProofV2Target::new(key).with_parent(parent))
    })
    .unwrap();
    assert_eq!(pending.len(), 2, "both operations need the opaque backing branch");
    assert_eq!(trie.root(epoch(2)), harness.original_root());
    assert_eq!(trie.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));
    assert_eq!(trie.take_updates(), SparseTrieUpdates::default());
    let (mut proof, _) = harness.proof_v2(&mut targets);
    trie.reveal_nodes(&mut proof).unwrap();
    harness.reveal_and_update(&mut trie, &mut pending);
    assert!(pending.is_empty());
    let mut reference = harness.init_trie_fully_revealed(true, new_trie);
    harness.reveal_and_update(&mut reference, &mut SuiteTestHarness::leaf_updates(&changes));
    assert_eq!(trie.root(epoch(3)), reference.root(epoch(3)));
    assert_eq!(trie.take_updates(), reference.take_updates());
}
