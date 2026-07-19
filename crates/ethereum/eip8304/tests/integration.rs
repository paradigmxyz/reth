use alloy_primitives::{Address, B256};
use reth_eip8304::{
    build_entries_for_block, encode_get_calldata, encode_set_calldata, IndexEntry, IndexTable,
    IndexTableStore,
};

fn make_topic(byte: u8) -> B256 {
    let mut t = B256::ZERO;
    t[0] = byte;
    t
}

fn fake_logs() -> Vec<(Address, Vec<B256>)> {
    let weth = Address::repeat_byte(0x01);
    let usdt = Address::repeat_byte(0x02);
    vec![
        (weth, vec![make_topic(0xd0), B256::repeat_byte(0xaa), B256::repeat_byte(0xbb)]),
        (usdt, vec![make_topic(0xd0), B256::repeat_byte(0xbb), B256::repeat_byte(0xaa)]),
    ]
}

fn block0_hash() -> B256 {
    B256::repeat_byte(0xbf)
}

#[test]
fn test_build_entries_for_block_matches_eip_example() {
    let tx_hash_0 = B256::repeat_byte(0xca);
    let tx_hash_1 = B256::repeat_byte(0xa7);

    let tx_hashes = vec![tx_hash_0, tx_hash_1];
    let receipt_logs = vec![fake_logs(), vec![]];

    let entries = build_entries_for_block(42, block0_hash(), &tx_hashes, &receipt_logs);

    assert!(!entries.is_empty());
    assert!(entries.iter().any(|e| e.entry_type == 0), "should have block entries");
    assert!(entries.iter().any(|e| e.entry_type == 1), "should have tx entries");
    assert!(entries.iter().any(|e| e.entry_type == 2), "should have address entries");
    assert!(entries.iter().any(|e| e.entry_type >= 3), "should have topic entries");
}

#[test]
fn test_block_entry_delay() {
    let entries = build_entries_for_block(0, B256::ZERO, &[], &[]);
    assert!(entries.iter().all(|e| e.entry_type != 0));

    let entries = build_entries_for_block(1, block0_hash(), &[B256::repeat_byte(0x42)], &[vec![]]);
    let block_entries: Vec<_> = entries.iter().filter(|e| e.entry_type == 0).collect();
    assert_eq!(block_entries.len(), 1);
    assert_eq!(block_entries[0].search_content, block0_hash());
    assert_eq!(block_entries[0].block_number, 0);
}

#[test]
fn test_encode_set_calldata() {
    let root = B256::repeat_byte(0xab);
    let calldata = encode_set_calldata(42, 1, root);
    assert_eq!(calldata.len(), 96);

    let first_block = u64::from_be_bytes(calldata[24..32].try_into().unwrap());
    let table_size = u64::from_be_bytes(calldata[56..64].try_into().unwrap());
    assert_eq!(first_block, 42);
    assert_eq!(table_size, 1);
    assert_eq!(&calldata[64..96], root.as_slice());
}

#[test]
fn test_encode_get_calldata() {
    let calldata = encode_get_calldata(42, 4);
    assert_eq!(calldata.len(), 64);

    let first_block = u64::from_be_bytes(calldata[24..32].try_into().unwrap());
    let table_size = u64::from_be_bytes(calldata[56..64].try_into().unwrap());
    assert_eq!(first_block, 42);
    assert_eq!(table_size, 4);
}

#[test]
fn test_lexicographic_ordering_with_mixed_types() {
    let weth = Address::repeat_byte(0x01);
    let topic0 = B256::repeat_byte(0xdd);

    let block_entry = IndexEntry::block_entry(40, B256::repeat_byte(0x01));
    let tx_entry = IndexEntry::transaction_entry(40, 0, B256::repeat_byte(0x01), 0);
    let address_entry = IndexEntry::log_address_entry(40, 0, 0, weth);
    let topic_entry = IndexEntry::log_topic_entry(40, 0, 0, 0, topic0);

    let mut entries = vec![topic_entry, address_entry, tx_entry, block_entry];
    entries.sort();

    assert_eq!(entries[0].entry_type, 0);
    assert_eq!(entries[1].entry_type, 1);
    assert!(entries[2].entry_type >= 2);
    assert!(entries[3].entry_type >= 2);
}

#[test]
fn test_store_and_merge() {
    let mut store = IndexTableStore::new(0);

    for i in 0..4 {
        let entry = IndexEntry::block_entry(i, B256::repeat_byte(i as u8));
        let table = IndexTable::new(i, 1, vec![entry]);
        store.store(table);
    }

    let merged = store.merge_level(0, 1).unwrap();
    assert_eq!(merged.first_block, 0);
    assert_eq!(merged.table_size, 4);
    assert_eq!(merged.entries.len(), 4);
}

#[test]
fn test_tables_to_generate_levels() {
    let mut store = IndexTableStore::new(0);

    for b in 0..16 {
        let entries = vec![IndexEntry::block_entry(b, B256::repeat_byte(b as u8))];
        store.store(IndexTable::new(b, 1, entries));
    }

    let pending = store.tables_to_generate_at(4);
    assert!(pending.contains(&(0, 4)));

    let pending = store.tables_to_generate_at(20);
    assert!(pending.contains(&(16, 4)));
}

#[test]
fn test_prune_keeps_recent_tables() {
    let mut store = IndexTableStore::new(0);
    for b in 0..600 {
        store.store(IndexTable::new(b, 1, vec![IndexEntry::block_entry(b, B256::ZERO)]));
    }
    store.prune(600);
    assert!(store.get(0, 1).is_none());
    assert!(store.get(80, 1).is_none());
    assert!(store.get(200, 1).is_some());
}
