use crate::{
    hashed_cursor::HashedPostStateCursorFactory, test_utils::storage_root_prehashed,
    updates::TrieUpdates, HashedPostState, HashedStorage, StorageRoot,
};
use reth_db::{
    cursor::DbCursorRW,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, trie::Nibbles, Address, StorageEntry, B256, U256};
use reth_provider::test_utils::create_test_provider_factory;
use reth_tracing::{
    tracing::info, tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer,
    Tracer,
};
use std::{collections::BTreeMap, iter};

#[test]
fn trie_updates_across_multiple_iterations() {
    let _ = RethTracer::new()
        .with_stdout(LayerInfo::new(
            LogFormat::Terminal,
            LevelFilter::TRACE.to_string(),
            "".to_string(),
            Some("always".to_string()),
        ))
        .init();
    let address = Address::ZERO;
    let hashed_address = keccak256(address);

    let factory = create_test_provider_factory();

    let mut hashed_storage = BTreeMap::default();
    let mut post_state = HashedPostState::default();

    // Block #1
    // Update specific storage slots
    let mut modified_storage = BTreeMap::default();

    // 0x0f..
    let modified_key_prefix = Nibbles::from_nibbles(
        [0x0, 0xf].into_iter().chain(iter::repeat(0).take(62)).collect::<Vec<_>>(),
    );

    // 0x0faa0..
    let mut modified_entry1 = modified_key_prefix.clone();
    modified_entry1.set_at(2, 0xa);
    modified_entry1.set_at(3, 0xa);

    // 0x0faaa..
    let mut modified_entry2 = modified_key_prefix.clone();
    modified_entry2.set_at(2, 0xa);
    modified_entry2.set_at(3, 0xa);
    modified_entry2.set_at(4, 0xa);

    // 0x0fab0..
    let mut modified_entry3 = modified_key_prefix.clone();
    modified_entry3.set_at(2, 0xa);
    modified_entry3.set_at(3, 0xb);

    // 0x0fba0..
    let mut modified_entry4 = modified_key_prefix.clone();
    modified_entry4.set_at(2, 0xb);
    modified_entry4.set_at(3, 0xa);

    [modified_entry1, modified_entry2, modified_entry3.clone(), modified_entry4]
        .into_iter()
        .for_each(|key| {
            modified_storage.insert(B256::from_slice(&key.pack()), U256::from(1));
        });

    // Update main hashed storage.
    hashed_storage.extend(modified_storage.clone());
    post_state.extend(HashedPostState::default().with_storages([(
        hashed_address,
        HashedStorage::from_iter(false, modified_storage.clone()),
    )]));

    let (storage_root, block1_updates) =
        compute_storage_root(address, factory.provider().unwrap().tx_ref(), &post_state);
    assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));
    println!("Block #1 Storage Root: {:?}", storage_root);
    println!("Block #1 Updates: {:#?}", block1_updates);
    block1_updates.clone().flush(factory.provider_rw().unwrap().tx_ref()).unwrap();

    // Block #2
    // Set 0x0fab0.. hashed slot to 0
    modified_storage.insert(B256::from_slice(&modified_entry3.pack()), U256::ZERO);

    // Update main hashed storage.
    hashed_storage.remove(&B256::from_slice(&modified_entry3.pack()));
    post_state.extend(HashedPostState::default().with_storages([(
        hashed_address,
        HashedStorage::from_iter(false, modified_storage.clone()),
    )]));

    let (storage_root, block2_updates) =
        compute_storage_root(address, factory.provider().unwrap().tx_ref(), &post_state);
    assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));
    println!("Block #2 Storage Root: {:?}", storage_root);

    // Commit trie updates
    {
        let mut updates = block1_updates.clone();
        updates.extend(block2_updates);

        let provider_rw = factory.provider_rw().unwrap();
        let mut hashed_storage_cursor =
            provider_rw.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
        for (hashed_slot, value) in &hashed_storage {
            hashed_storage_cursor
                .upsert(hashed_address, StorageEntry { key: *hashed_slot, value: *value })
                .unwrap();
        }
        // updates.flush(provider_rw.tx_ref()).unwrap();
        provider_rw.commit().unwrap();
    }

    // Recompute storage root for block #3
    let storage_root =
        StorageRoot::from_tx(factory.provider().unwrap().tx_ref(), address).root().unwrap();
    assert_eq!(storage_root, storage_root_prehashed(hashed_storage.clone()));
}

fn compute_storage_root<TX: DbTx>(
    address: Address,
    tx: &TX,
    post_state: &HashedPostState,
) -> (B256, TrieUpdates) {
    let mut prefix_sets = post_state.construct_prefix_sets();
    let (root, _, updates) = StorageRoot::from_tx(tx, address)
        .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
            tx,
            &post_state.clone().into_sorted(),
        ))
        .with_prefix_set(prefix_sets.storage_prefix_sets.remove(&keccak256(address)).unwrap())
        .root_with_updates()
        .unwrap();
    (root, updates)
}
