use super::*;

fn key_with_prefix(bytes: &[u8]) -> B256 {
    let mut key = B256::ZERO;
    key.0[..bytes.len()].copy_from_slice(bytes);
    key
}

pub(super) fn test_changed_paths_record_base_paths_for_branches_and_leaves<T: SparseTrie>(
    new_trie: fn() -> T,
) {
    let mut trie = new_trie();
    trie.set_changed_paths(true);

    let key_a = key_with_prefix(&[0x12, 0x34]);
    let key_b = key_with_prefix(&[0x12, 0x35]);

    let mut updates = B256Map::default();
    updates.insert(key_a, LeafUpdate::Changed(vec![0x01; 64]));
    updates.insert(key_b, LeafUpdate::Changed(vec![0x02; 64]));
    trie.update_leaves(&mut updates, |_, _| {}).expect("insertion should succeed");
    assert!(updates.is_empty());

    let _ = trie.root();

    let changed_paths = trie.take_changed_paths();
    assert!(!changed_paths.contains(&Nibbles::default()));
    assert!(changed_paths.contains(&Nibbles::from_nibbles([0x01, 0x02, 0x03, 0x04])));
    assert!(changed_paths.contains(&Nibbles::from_nibbles([0x01, 0x02, 0x03, 0x05])));
    assert!(!changed_paths.contains(&Nibbles::from_nibbles([0x01, 0x02, 0x03])));
    assert!(!changed_paths.contains(&Nibbles::unpack(key_a)));
    assert!(!changed_paths.contains(&Nibbles::unpack(key_b)));

    let _ = trie.root();
    assert!(trie.take_changed_paths().is_empty());
}

pub(super) fn test_changed_paths_skip_dirty_ancestor_branch_when_descendant_changed<
    T: SparseTrie,
>(
    new_trie: fn() -> T,
) {
    let mut trie = new_trie();
    trie.set_changed_paths(true);

    let key_a = key_with_prefix(&[0xff, 0x10]);
    let key_b = key_with_prefix(&[0xff, 0x20]);
    let sibling_key = key_with_prefix(&[0xe0]);

    let mut updates = B256Map::default();
    updates.insert(key_a, LeafUpdate::Changed(vec![0x01; 64]));
    updates.insert(key_b, LeafUpdate::Changed(vec![0x02; 64]));
    updates.insert(sibling_key, LeafUpdate::Changed(vec![0x03; 64]));
    trie.update_leaves(&mut updates, |_, _| {}).expect("insertion should succeed");
    assert!(updates.is_empty());

    let _ = trie.root();
    let _ = trie.take_changed_paths();

    let mut updates = B256Map::from_iter([(key_a, LeafUpdate::Changed(vec![0x04; 64]))]);
    trie.update_leaves(&mut updates, |_, _| {}).expect("update should succeed");
    assert!(updates.is_empty());

    let _ = trie.root();

    let changed_paths = trie.take_changed_paths();
    assert!(changed_paths.contains(&Nibbles::from_nibbles([0x0f, 0x0f, 0x01])));
    assert!(!changed_paths.contains(&Nibbles::from_nibbles([0x0f])));
    assert!(!changed_paths.contains(&Nibbles::default()));

    let mut updates = B256Map::from_iter([(key_b, LeafUpdate::Changed(vec![0x05; 64]))]);
    trie.update_leaves(&mut updates, |_, _| {}).expect("update should succeed");
    assert!(updates.is_empty());

    let _ = trie.root();

    let changed_paths = trie.take_changed_paths();
    assert!(changed_paths.contains(&Nibbles::from_nibbles([0x0f, 0x0f, 0x02])));
    assert!(!changed_paths.contains(&Nibbles::from_nibbles([0x0f, 0x0f, 0x01])));
    assert!(!changed_paths.contains(&Nibbles::from_nibbles([0x0f])));
    assert!(!changed_paths.contains(&Nibbles::default()));
}

pub(super) fn test_changed_paths_record_removed_subtrie_leaf_and_collapsed_parent_branch<
    T: SparseTrie,
>(
    new_trie: fn() -> T,
) {
    let mut trie = new_trie();
    trie.set_changed_paths(true);

    let removed_key = key_with_prefix(&[0x12]);
    let retained_key = key_with_prefix(&[0x13]);

    let mut updates = B256Map::default();
    updates.insert(removed_key, LeafUpdate::Changed(vec![0x01]));
    updates.insert(retained_key, LeafUpdate::Changed(vec![0x02]));
    trie.update_leaves(&mut updates, |_, _| {}).expect("insertion should succeed");
    assert!(updates.is_empty());
    assert!(trie.take_changed_paths().is_empty());

    let mut removals = B256Map::default();
    removals.insert(removed_key, LeafUpdate::Changed(Vec::new()));
    trie.update_leaves(&mut removals, |_, _| {}).expect("removal should succeed");
    assert!(removals.is_empty());

    let _ = trie.root();

    let changed_paths = trie.take_changed_paths();
    assert!(changed_paths.contains(&Nibbles::from_nibbles([0x01, 0x02])));
    assert!(changed_paths.contains(&Nibbles::default()));

    let _ = trie.root();
    assert!(trie.take_changed_paths().is_empty());
}
