use alloy_primitives::{
    map::{B256HashMap, B256HashSet, HashMap},
    B256, U256,
};
use alloy_rlp::Decodable;
use reth_db::{tables, transaction::DbTx};
use reth_primitives::StorageEntry;
use reth_provider::{
    test_utils::create_test_provider_factory, BlockWriter, StateWriter, StorageTrieWriter,
};
use reth_trie::{
    hashed_cursor::{self, HashedCursorFactory},
    prefix_set::PrefixSetMut,
    proof::StorageProof,
    trie_cursor::TrieCursorFactory,
    HashedPostStateSorted, HashedStorage, Nibbles, StorageRoot, TrieNode,
};
use reth_trie_db::{
    DatabaseHashedAccountCursor, DatabaseHashedCursorFactory, DatabaseHashedStorageCursor,
    DatabaseStorageTrieCursor, DatabaseTrieCursorFactory, SparseTrieBuilder,
};
use reth_trie_sparse::SparseTrie;
use std::collections::BTreeMap;

fn compare<T, H>(
    trie_cursor_factory: T,
    hashed_cursor_factory: H,
    hashed_address: B256,
    targets: B256HashSet,
    node_comparison_enabled: bool,
) where
    T: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    let storage_multiproof = StorageProof::new_hashed(
        trie_cursor_factory.clone(),
        hashed_cursor_factory.clone(),
        hashed_address,
    )
    .storage_multiproof(targets.clone())
    .unwrap();
    println!("storage_multiproof {:?}", storage_multiproof);

    let mut subtree = storage_multiproof.subtree.into_nodes_sorted().into_iter();
    let root_node_encoded = subtree.next().unwrap().1;
    let root = TrieNode::decode(&mut &root_node_encoded[..]).unwrap();

    let mut expected = SparseTrie::blind();
    let expected_revealed = expected.reveal_root(root.clone(), None, None, false).unwrap();

    for (path, bytes) in subtree {
        let node = TrieNode::decode(&mut &bytes[..]).unwrap();
        expected_revealed.reveal_node(path, node, None, None).unwrap();
    }
    println!("sparse {expected_revealed:?}");

    // let mut computed = SparseTrie::revealed_empty();
    // let computed_revealed = computed.as_revealed_mut().unwrap();
    let trie_cursor = trie_cursor_factory.storage_trie_cursor(hashed_address).unwrap();
    let hashed_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address).unwrap();
    let prefix_set = PrefixSetMut::from(targets.into_iter().map(Nibbles::unpack)).freeze();
    // reveal_trie_from_cursors(
    //     computed_revealed,
    //     trie_cursor,
    //     hashed_cursor,
    //     PrefixSetMut::from(targets.into_iter().map(Nibbles::unpack)).freeze(),
    // )
    // .unwrap();
    let mut computed_revealed =
        SparseTrieBuilder::build(trie_cursor, hashed_cursor, prefix_set).unwrap();
    println!("sparse 2 {computed_revealed:?}");

    if node_comparison_enabled {
        similar_asserts::assert_eq!(
            BTreeMap::from_iter(expected_revealed.nodes_ref().clone()),
            BTreeMap::from_iter(computed_revealed.nodes_ref().clone())
        );
    }
    assert_eq!(expected_revealed.root(), computed_revealed.root());
}

#[test]
fn test() {
    let hashed_address = B256::ZERO;
    // TODO: panic on (1..=20)
    let keys = (1..=32).map(B256::with_last_byte).collect::<B256HashSet>();
    let storage = HashedStorage::from_iter(false, keys.iter().map(|key| (*key, U256::from(1))));

    let factory = create_test_provider_factory();

    // Write storage slots to the database.
    let provider_rw = factory.provider_rw().unwrap();
    provider_rw
        .write_hashed_state(&HashedPostStateSorted::new(
            Default::default(),
            B256HashMap::from_iter([(hashed_address, storage.clone().into_sorted())]),
        ))
        .unwrap();

    let trie_cursor_factory = DatabaseTrieCursorFactory::new(provider_rw.tx_ref());
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(provider_rw.tx_ref());

    let (_, _, trie_updates) = StorageRoot::new_hashed(
        trie_cursor_factory.clone(),
        hashed_cursor_factory.clone(),
        hashed_address,
        storage.construct_prefix_set().freeze(),
        Default::default(),
    )
    .root_with_updates()
    .unwrap();

    provider_rw
        .write_storage_trie_updates(&HashMap::from_iter([(hashed_address, trie_updates)]))
        .unwrap();

    // All targets.
    compare(
        trie_cursor_factory.clone(),
        hashed_cursor_factory.clone(),
        hashed_address,
        keys,
        false,
    );
    println!("\n\n\n");
    // One target.
    compare(
        trie_cursor_factory,
        hashed_cursor_factory,
        hashed_address,
        B256HashSet::from_iter([B256::with_last_byte(1)]),
        true,
    );
}
