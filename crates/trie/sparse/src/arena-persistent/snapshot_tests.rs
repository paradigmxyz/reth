use super::*;
use reth_trie_common::LeafNode;

const FIRST: B256 = key(0x11, 0x00);
const NEIGHBOR: B256 = key(0x11, 0x10);
const OTHER: B256 = key(0x12, 0x00);
const FAR: B256 = key(0x20, 0x00);

const fn key(first: u8, second: u8) -> B256 {
    let mut bytes = [0; 32];
    bytes[0] = first;
    bytes[1] = second;
    B256::new(bytes)
}

fn apply(trie: &mut ArenaParallelSparseTrie, updates: &[(B256, &[u8])]) {
    let mut changes =
        updates.iter().map(|(key, value)| (*key, LeafUpdate::Changed(value.to_vec()))).collect();
    trie.update_leaves(&mut changes, |key, parent| panic!("unexpected proof: {key} {parent:?}"))
        .unwrap();
    assert!(changes.is_empty());
}

fn fixture() -> ArenaParallelSparseTrie {
    let mut trie = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
        ArenaParallelismThresholds {
            min_revealed_nodes: 0,
            min_updates: 0,
            min_dirty_leaves: 0,
            min_leaves_for_prune: 0,
        },
    );
    trie.set_updates(true);
    apply(
        &mut trie,
        &[(FIRST, &[1; 40]), (NEIGHBOR, &[2; 40]), (OTHER, &[3; 40]), (FAR, &[4; 40])],
    );
    trie
}

fn leaf(trie: &ArenaParallelSparseTrie, key: B256) -> &Vec<u8> {
    trie.get_leaf_value(&Nibbles::unpack(key)).unwrap()
}

#[test]
fn forks_share_untouched_leaves_and_can_both_be_updated() {
    let mut original = fixture();
    let epoch = TrieNodeEpoch::new(1);
    let root = original.root(epoch);
    let mut expected = original.clone();

    let mut fork = original.snapshot();
    assert_eq!(fork.root_epoch(), Some(epoch));
    assert_eq!(original.updates_ref(), fork.updates_ref());
    assert!(!fork.take_updates().updated_nodes.is_empty());
    assert!(!original.updates_ref().updated_nodes.is_empty());

    let edits: &[(B256, &[u8])] = &[(FIRST, &[5; 40]), (FAR, &[6; 40])];
    apply(&mut fork, edits);
    apply(&mut expected, edits);
    let epoch = TrieNodeEpoch::new(2);
    assert_eq!(fork.root(epoch), expected.root(epoch));
    assert_eq!(leaf(&original, FIRST), &[1; 40]);
    assert_eq!(leaf(&fork, FIRST), &[5; 40]);
    assert!(core::ptr::eq(leaf(&original, NEIGHBOR), leaf(&fork, NEIGHBOR)));
    assert!(core::ptr::eq(leaf(&original, OTHER), leaf(&fork, OTHER)));
    assert_eq!(original.root(epoch), root);

    // Fork an already-diverged version, then change an edited slot and reuse a removed one.
    let mut next = fork.snapshot();
    apply(&mut fork, &[(FIRST, &[7; 40]), (NEIGHBOR, &[]), (key(0x11, 0x20), &[9; 40])]);
    apply(&mut original, &[(OTHER, &[8; 40])]);
    assert_eq!(leaf(&next, FIRST), &[5; 40]);
    assert_eq!(leaf(&next, NEIGHBOR), &[2; 40]);
    assert_eq!(leaf(&next, OTHER), &[3; 40]);
    assert_eq!(next.root(epoch), expected.root(epoch));

    drop(fork);
    drop(original);
    apply(&mut next, &[(FIRST, &[10; 40]), (FAR, &[])]);
    apply(&mut expected, &[(FIRST, &[10; 40]), (FAR, &[])]);
    let epoch = TrieNodeEpoch::new(3);
    assert_eq!(next.root(epoch), expected.root(epoch));
}

#[test]
fn pruning_a_fork_keeps_other_versions_revealed() {
    let mut original = fixture();
    original.root(TrieNodeEpoch::new(1));
    let mut expected = original.clone();
    let mut fork = original.snapshot();

    for trie in [&mut fork, &mut expected] {
        apply(trie, &[(FIRST, &[5; 40])]);
    }
    let epoch = TrieNodeEpoch::new(2);
    let fork_root = fork.root(epoch);
    assert_eq!(fork_root, expected.root(epoch));
    assert_ne!(fork_root, original.root(epoch));
    let before = leaf(&fork, FIRST).as_ptr();
    let pruned = fork.prune(epoch);
    assert!(pruned > 0);
    assert_eq!(pruned, expected.prune(epoch));
    assert_eq!(fork.prune(epoch), 0);
    assert_eq!(before, leaf(&fork, FIRST).as_ptr());
    assert_eq!(fork.root(epoch), fork_root);
    assert_eq!(leaf(&original, NEIGHBOR), &[2; 40]);
    assert_eq!(leaf(&original, OTHER), &[3; 40]);
    assert_eq!(leaf(&original, FAR), &[4; 40]);

    let absent = Nibbles::unpack(NEIGHBOR);
    assert_eq!(fork.find_leaf(&absent, None), expected.find_leaf(&absent, None));
    assert!(matches!(fork.find_leaf(&absent, None), Err(LeafLookupError::BlindedNode { .. })));

    let mut proof = [ProofTrieNodeV2 {
        path: absent.slice(..3),
        node: TrieNodeV2::Leaf(LeafNode::new(absent.slice(3..), vec![2; 40])),
        masks: None,
    }];
    fork.reveal_nodes(&mut proof).unwrap();
    assert_eq!(leaf(&fork, NEIGHBOR), &[2; 40]);
    assert_eq!(fork.root(epoch), fork_root);

    fork.clear();
    assert_eq!(fork.root(epoch), EMPTY_ROOT_HASH);
    assert_eq!(leaf(&original, FIRST), &[1; 40]);
}
