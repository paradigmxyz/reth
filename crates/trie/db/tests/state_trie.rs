//! Complete node persistence, overlay cursors, and proof reconstruction.
use alloy_primitives::{keccak256, B256, U256};
use reth_db::{test_utils::create_test_rw_db, Database};
use reth_db_api::{tables, transaction::DbTx};
use reth_trie::{
    proof_v3::ProofCalculator,
    state_trie_cursor::{
        InMemoryStateTrieCursor, StateTrieCursor, StateTrieCursorFactory, StateTrieStorageCursor,
    },
    Nibbles, ProofV2Target, StateTrieBuilder, StateTrieNode, StateTrieUpdatesSorted, TrieAccount,
};
use reth_trie_db::{write_state_trie_updates, DatabaseStateTrieCursorFactory};
use std::collections::BTreeMap;

#[test]
fn database_and_overlay_cursors_preserve_nodes_and_deletions() {
    let db = create_test_rw_db();
    let address = B256::repeat_byte(1);
    let other_address = B256::repeat_byte(2);
    let mut builder = StateTrieBuilder::default();
    let storage: BTreeMap<_, _> =
        (1u64..512).map(|i| (keccak256(i.to_be_bytes()), U256::from(i))).collect();
    let mut nodes = BTreeMap::new();
    let mut write = |p, n| {
        nodes.insert(p, n);
        Ok::<_, core::convert::Infallible>(())
    };
    for (k, v) in &storage {
        builder.push(*k, *v, &mut write).unwrap();
    }
    let root = builder.finish(&mut write).unwrap();
    let mut updates = StateTrieUpdatesSorted::default();
    updates
        .storage_tries
        .insert(address, nodes.iter().map(|(p, n)| (*p, Some(n.clone()))).collect());
    let account_path = Nibbles::unpack(address);
    updates.account_nodes.push((
        account_path,
        Some(StateTrieNode::Leaf {
            short_key_len: 64,
            value: TrieAccount { storage_root: root, ..Default::default() },
        }),
    ));
    let tx = db.tx_mut().unwrap();
    write_state_trie_updates(&tx, &updates).unwrap();
    tx.commit().unwrap();
    let tx = db.tx().unwrap();
    let factory = DatabaseStateTrieCursorFactory(&tx);
    let mut cursor = factory.state_trie_storage_cursor(address).unwrap();
    for (p, n) in &nodes {
        assert_eq!(cursor.get(*p).unwrap().as_ref(), Some(n));
        assert_eq!(cursor.seek(*p).unwrap(), Some((*p, n.clone())));
        assert_eq!(
            cursor.before(Some(*p)).unwrap(),
            nodes.range(..p).next_back().map(|(p, n)| (*p, n.clone()))
        );
    }
    cursor.set_hashed_address(other_address);
    assert!(cursor.before(None).unwrap().is_none());
    assert!(cursor.get(Nibbles::new()).unwrap().is_none());
    cursor.set_hashed_address(address);
    let mut proof = ProofCalculator::new_storage(cursor);
    let node = proof.storage_root_node(address).unwrap();
    assert_eq!(proof.compute_root_hash(&[node]).unwrap(), Some(root));
    let keys: Vec<_> = storage.keys().take(12).copied().collect();
    let proofs = proof
        .storage_proof(
            address,
            &mut keys.iter().map(|k| ProofV2Target::new(*k)).collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(proof.compute_root_hash(&proofs).unwrap(), Some(root));

    let mut overlay = StateTrieUpdatesSorted::default();
    let changes: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 3 == 0)
        .map(|(_, (p, _))| (*p, None))
        .collect();
    for (p, _) in &changes {
        nodes.remove(p);
    }
    overlay.storage_tries.insert(address, changes);
    let mut cursor = InMemoryStateTrieCursor::new_storage(
        factory.state_trie_storage_cursor(address).unwrap(),
        &overlay,
        address,
    );
    for (p, n) in &nodes {
        assert_eq!(cursor.get(*p).unwrap().as_ref(), Some(n));
        assert_eq!(
            cursor.before(Some(*p)).unwrap(),
            nodes.range(..p).next_back().map(|(p, n)| (*p, n.clone()))
        );
    }
    for (p, _) in &overlay.storage_tries[&address] {
        assert!(cursor.get(*p).unwrap().is_none());
        assert_eq!(cursor.seek(*p).unwrap(), nodes.range(p..).next().map(|(p, n)| (*p, n.clone())));
    }
    drop(cursor);
    drop(proof);
    drop(tx);
    let tx = db.tx_mut().unwrap();
    write_state_trie_updates(&tx, &overlay).unwrap();
    assert_eq!(tx.entries::<tables::StateTrieStorages>().unwrap(), nodes.len());
}

#[test]
fn masked_persistence_leaves_suffix_changes_in_the_overlay() {
    let db = create_test_rw_db();
    let path = Nibbles::unpack(B256::ZERO);
    let mk = |value: Option<U256>| {
        let mut u = StateTrieUpdatesSorted::default();
        u.storage_tries.insert(
            B256::ZERO,
            vec![(path, value.map(|value| StateTrieNode::Leaf { short_key_len: 64, value }))],
        );
        u
    };
    let original = mk(Some(U256::from(1)));
    let changed = mk(Some(U256::from(2)));
    let deleted = mk(None);
    let tx = db.tx_mut().unwrap();
    write_state_trie_updates(&tx, &original).unwrap();
    let masked = StateTrieUpdatesSorted::disjointed_merge_batch(&[&changed], &[&deleted]);
    write_state_trie_updates(&tx, &masked).unwrap();
    let factory = DatabaseStateTrieCursorFactory(&tx);
    assert!(
        matches!(factory.state_trie_storage_cursor(B256::ZERO).unwrap().get(path).unwrap(),Some(StateTrieNode::Leaf { value, .. }) if value == U256::from(1))
    );
    let mut cursor = InMemoryStateTrieCursor::new_storage(
        factory.state_trie_storage_cursor(B256::ZERO).unwrap(),
        &deleted,
        B256::ZERO,
    );
    assert!(cursor.get(path).unwrap().is_none());
    assert!(cursor.seek(Nibbles::new()).unwrap().is_none());
}
