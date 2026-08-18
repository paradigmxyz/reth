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

fn block0_hash() -> B256 {
    B256::repeat_byte(0xbf)
}

/// Reconstructs the block structure of the EIP-8304 worked example (blocks #40–#43):
/// blocks #40 and #41 are empty; #42 has 2 transactions and 3 logs; #43 has 1
/// transaction and 1 log. With the one-block block-entry delay this yields a size-4
/// table (first block #40) of 21 entries: 4 block, 3 transaction, and 14 log entries
/// (4 log addresses + 10 topics).
///
/// TODO(exact-hex): pin each entry's binary encoding against the hex strings in the
/// EIP worked-example table. This test currently pins the entry counts, the delayed
/// block-entry set, cumulative log counts, and sorted ordering, which are a much
/// stronger oracle than the previous "each type is present" check.
#[test]
fn test_build_entries_for_block_matches_eip_example() {
    // parent hashes: block N carries the block entry of N-1 with N-1's hash.
    let h = |b: u8| B256::repeat_byte(b);

    // Block #42: tx0 -> 2 logs (3 + 2 topics), tx1 -> 1 log (3 topics) = 3 logs.
    let addr = |b: u8| Address::repeat_byte(b);
    let logs_42: Vec<Vec<(Address, Vec<B256>)>> = vec![
        vec![
            (addr(0x01), vec![make_topic(0xd0), make_topic(0xa1), make_topic(0xa2)]),
            (addr(0x02), vec![make_topic(0xd0), make_topic(0xb1)]),
        ],
        vec![(addr(0x03), vec![make_topic(0xd0), make_topic(0xc1), make_topic(0xc2)])],
    ];
    // Block #43: tx0 -> 1 log (2 topics).
    let logs_43: Vec<Vec<(Address, Vec<B256>)>> =
        vec![vec![(addr(0x04), vec![make_topic(0xd0), make_topic(0xe1)])]];

    let mut table = Vec::new();
    table.extend(build_entries_for_block(40, h(39), &[], &[]));
    table.extend(build_entries_for_block(41, h(40), &[], &[]));
    table.extend(build_entries_for_block(
        42,
        h(41),
        &[B256::repeat_byte(0xca), B256::repeat_byte(0xa7)],
        &logs_42,
    ));
    table.extend(build_entries_for_block(43, h(42), &[B256::repeat_byte(0xd7)], &logs_43));
    table.sort();

    // Entry-count breakdown from the worked example.
    let n_block = table.iter().filter(|e| e.entry_type == 0).count();
    let n_tx = table.iter().filter(|e| e.entry_type == 1).count();
    let n_log = table.iter().filter(|e| e.entry_type >= 2).count();
    assert_eq!(n_block, 4, "block entries #39..#42");
    assert_eq!(n_tx, 3, "2 txs in #42 + 1 tx in #43");
    assert_eq!(n_log, 14, "4 log addresses + 10 topics");
    assert_eq!(table.len(), 21);

    // The delayed block entries are exactly the parents #39..#42 with their hashes.
    let mut block_entries: Vec<_> = table.iter().filter(|e| e.entry_type == 0).collect();
    block_entries.sort_by_key(|e| e.block_number);
    for (i, b) in (39u64..=42).enumerate() {
        assert_eq!(block_entries[i].block_number, b);
        assert_eq!(block_entries[i].search_content, h(b as u8));
    }

    // Cumulative log count recorded on each tx entry is the count of logs before it.
    let mut tx_entries: Vec<_> = table.iter().filter(|e| e.entry_type == 1).collect();
    tx_entries.sort_by_key(|e| (e.block_number, e.tx_index));
    assert_eq!((tx_entries[0].block_number, tx_entries[0].log_index), (42, 0)); // #42 tx0
    assert_eq!((tx_entries[1].block_number, tx_entries[1].log_index), (42, 2)); // #42 tx1, 2 logs before
    assert_eq!((tx_entries[2].block_number, tx_entries[2].log_index), (43, 0)); // #43 tx0

    // Table is sorted by binary encoding.
    for i in 1..table.len() {
        assert!(table[i - 1] <= table[i]);
    }
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
