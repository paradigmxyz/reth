use super::*;
use alloy_primitives::{Address, B256, U256};
use reth_db::{
    init_db,
    mdbx::DatabaseArguments,
    open_db,
    tables::{HeaderNumbers, PackedAccountsTrie, PackedStoragesTrie, PlainStorageState},
    transaction::DbTxMut,
    Database, RawDupSort, RawTable, RawValue,
};
use reth_primitives_traits::StorageEntry;
use serde_json::{json, Value};

fn db_get_mdbx(datadir: &str, table: &str, key: &str, args: &[&str]) -> String {
    let mut cmd = vec!["db", "--chain", "dev", "--datadir", datadir, "get", "mdbx", table, key];
    cmd.extend_from_slice(args);
    reth_ok(&cmd)
}

#[test]
fn db_get_mdbx_subkey_points_and_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let datadir = dir.path().to_str().unwrap();

    let address = Address::with_last_byte(0xaa);
    let next_address = Address::with_last_byte(0xcc);
    let next_entry = StorageEntry::new(B256::ZERO, U256::ONE);
    let stored = B256::with_last_byte(3);
    // Include a leading zero in the stored integer to distinguish raw output from re-encoding.
    let raw = [stored.as_slice(), &[0, 0x21]].concat();
    init_db(dir.path().join("db"), DatabaseArguments::test())
        .unwrap()
        .update(|tx| {
            tx.put::<PlainStorageState>(
                address,
                StorageEntry::new(B256::with_last_byte(1), U256::from(11)),
            )?;
            tx.put::<RawDupSort<PlainStorageState>>(address.into(), RawValue::from_vec(raw))?;
            tx.put::<PlainStorageState>(next_address, next_entry)
        })
        .unwrap()
        .unwrap();

    let key = address.to_string();
    let get = |args: &[&str]| db_get_mdbx(datadir, "PlainStorageState", &key, args);

    let gap = B256::with_last_byte(2).to_string();
    let end = B256::with_last_byte(4).to_string();
    assert_eq!(get(&[&gap]), "");
    assert_eq!(get(&[&end]), "");

    let present = stored.to_string();
    let entry = json!({ "key": stored, "value": "0x21" });
    assert_eq!(serde_json::from_str::<Value>(&get(&[&present])).unwrap(), entry);
    assert_eq!(get(&[&present, "--raw"]), format!("{present}0021\n"));
    assert_eq!(get(&[&gap, &key, &present, "--raw"]), "");
    assert_eq!(
        serde_json::from_str::<Value>(&get(&[&present, &key, &end, "--raw"])).unwrap(),
        json!({ "key": address, "val": format!("{present}0021") })
    );

    let range = get(&[&gap, &key, &end]);
    assert_eq!(
        serde_json::from_str::<Value>(&range).unwrap(),
        json!({ "key": address, "val": entry })
    );

    let upper = Address::with_last_byte(0xdd).to_string();
    let range_from = |start: Address| {
        db_get_mdbx(datadir, "PlainStorageState", &start.to_string(), &[&end, &upper])
    };
    // Both an exhausted address and a missing address start at the next account's first slot.
    for start in [address, Address::with_last_byte(0xbb)] {
        assert_eq!(
            serde_json::from_str::<Value>(&range_from(start)).unwrap(),
            json!({ "key": next_address, "val": next_entry })
        );
    }
    assert_eq!(range_from(next_address), "");
}

#[test]
fn db_get_mdbx_packed_trie_keys() {
    let dir = tempfile::tempdir().unwrap();
    let datadir = dir.path().to_str().unwrap();
    reth_ok(&["init", "--chain", "dev", "--datadir", datadir, "--storage.v2=true"]);

    let key = B256::with_last_byte(1);
    let entry = json!({
        "nibbles": "0xa0",
        "node": { "state_mask": 3, "tree_mask": 0, "hash_mask": 0, "hashes": [], "root_hash": null }
    });
    open_db(dir.path().join("db"), DatabaseArguments::test())
        .unwrap()
        .update(|tx| {
            tx.put::<PackedAccountsTrie>(
                serde_json::from_value(entry["nibbles"].clone()).unwrap(),
                serde_json::from_value(entry["node"].clone()).unwrap(),
            )?;
            tx.put::<PackedStoragesTrie>(key, serde_json::from_value(entry.clone()).unwrap())
        })
        .unwrap()
        .unwrap();

    let account = |args: &[&str]| db_get_mdbx(datadir, "AccountsTrie", "0xa0", args);
    assert_eq!(serde_json::from_str::<Value>(&account(&[])).unwrap(), entry["node"]);
    assert_eq!(
        serde_json::from_str::<Value>(&account(&["0xb"])).unwrap(),
        json!({ "key": "0xa0", "val": entry["node"] })
    );

    let key = key.to_string();
    let get = |args: &[&str]| db_get_mdbx(datadir, "StoragesTrie", &key, args);
    assert_eq!(serde_json::from_str::<Value>(&get(&["0xa0"])).unwrap(), entry);
    // The packed paths for 0xa and 0xa0 differ only in the encoded nibble count.
    assert_eq!(get(&["0xa", "--raw"]), "");
    let range = get(&["0xa", &key, "0xb"]);
    assert_eq!(serde_json::from_str::<Value>(&range).unwrap(), json!({ "key": key, "val": entry }));
}

#[test]
fn db_get_mdbx_raw_table_range() {
    let dir = tempfile::tempdir().unwrap();
    let key = B256::with_last_byte(1);
    // The block number decoder accepts trailing bytes that re-encoding would drop.
    let raw = [33u64.to_le_bytes().as_slice(), &[0xff]].concat();
    init_db(dir.path().join("db"), DatabaseArguments::test())
        .unwrap()
        .update(|tx| tx.put::<RawTable<HeaderNumbers>>(key.into(), RawValue::from_vec(raw)))
        .unwrap()
        .unwrap();

    let range = db_get_mdbx(
        dir.path().to_str().unwrap(),
        "HeaderNumbers",
        &key.to_string(),
        &[&B256::with_last_byte(2).to_string(), "--raw"],
    );
    assert_eq!(
        serde_json::from_str::<Value>(&range).unwrap(),
        json!({ "key": key, "val": "0x2100000000000000ff" })
    );
}
